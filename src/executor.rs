use emscripten_rs_sys as ffi;
use futures::task::ArcWake;
use send_wrapper::SendWrapper;
use std::{cell::Cell, ffi::c_void, ptr::null_mut, sync::Arc};

mod local_pool;
pub use local_pool::*;

#[cfg(test)]
mod tests;

unsafe extern "C" {
    // Available in the SDK but not yet exposed by emscripten_rs_sys.
    // Unlike promise_await, this does not create a .then() promise to capture
    // rejection. Our notification promises are only ever fulfilled.
    fn emscripten_promise_await_unchecked(promise: ffi::em_promise_t) -> *mut c_void;
}

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
        let state = &*self.state;
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
        let state = &*self.state;
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
        unsafe { emscripten_promise_await_unchecked(promise) };
        state.promise.set(null_mut());
        unsafe { ffi::emscripten_promise_destroy(promise) };
        state.notified.set(false);
    }

    fn woken(&self) -> bool {
        self.state.notified.get()
    }
}

impl ArcWake for PromiseWaker {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.notify();
    }
}
