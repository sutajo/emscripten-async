use emscripten_rs_sys as ffi;
use futures::task::ArcWake;
use std::{cell::Cell, ptr::null_mut, sync::Arc};

mod local_pool;
pub use local_pool::*;

use crate::send_wrapper::SendWrapper;

#[cfg(test)]
mod tests;

// Multiple wakers, one waiter, all on the executor's thread. A notification
// received before wait() is consumed without allocating a native promise.
struct PromiseWaker {
    state: SendWrapper<PromiseWakerState>,
}

#[derive(Default)]
struct PromiseWakerState {
    promise: Cell<ffi::em_promise_t>,
    notified: Cell<bool>,
}

impl Default for PromiseWaker {
    fn default() -> Self {
        Self {
            state: SendWrapper::new(PromiseWakerState::default()),
        }
    }
}

impl PromiseWaker {
    fn notify(&self) {
        let state = self.state.as_ref();
        if state.notified.replace(true) {
            // Already notified
            return;
        }

        let promise = state.promise.get();
        if !promise.is_null() {
            // There is a waiter
            unsafe {
                ffi::emscripten_promise_resolve(
                    promise,
                    ffi::em_promise_result_t_EM_PROMISE_FULFILL,
                    null_mut(),
                );
            }
        }
    }

    fn wait(&self) {
        let state = self.state.as_ref();
        assert!(
            state.promise.get().is_null(),
            "only one waiter is supported"
        );
        if state.notified.replace(false) {
            // Already notified, don't suspend
            return;
        }

        let promise = unsafe { ffi::emscripten_promise_create() };
        assert!(!promise.is_null());
        state.promise.set(promise);
        // No borrow is held across suspension. Wakeups resolve this handle;
        // additional wakeups before resuming are coalesced by notified.
        // Unlike promise_await, this does not create a .then() promise to capture
        // rejection. Our notification promises are only ever fulfilled.
        unsafe { ffi::emscripten_promise_await_unchecked(promise) };
        state.promise.set(null_mut());
        unsafe { ffi::emscripten_promise_destroy(promise) };
        state.notified.set(false);
    }

    fn woken(&self) -> bool {
        self.state.as_ref().notified.get()
    }
}

impl ArcWake for PromiseWaker {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.notify();
    }
}
