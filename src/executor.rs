use emscripten_functions_sys::emscripten as ffi;
use futures::task::ArcWake;
use std::{cell::Cell, ffi::c_void, ptr::null_mut, sync::Arc, task::Wake};

mod local_pool;
pub use local_pool::*;

#[cfg(test)]
mod tests;

unsafe extern "C" {
    // Available in the SDK but not yet exposed by emscripten-functions-sys.
    // Unlike promise_await, this does not create a .then() promise to capture
    // rejection. Our notification promises are only ever fulfilled.
    fn emscripten_promise_await_unchecked(promise: ffi::em_promise_t) -> *mut c_void;
}

// Multiple wakers, one waiter, all on the executor's thread. A notification
// received before wait() is consumed without allocating a native promise.
#[derive(Default)]
struct PromiseWaker {
    promise: Cell<ffi::em_promise_t>,
    notified: Cell<bool>,
}

// Waker/ArcWake require Send + Sync, but this executor and all of its wakeups
// must stay on the same thread, as documented by the crate.
unsafe impl Send for PromiseWaker {}
unsafe impl Sync for PromiseWaker {}

impl PromiseWaker {
    fn notify(&self) {
        if self.notified.replace(true) {
            // Already notified
            return;
        }

        let promise = self.promise.get();
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
        assert!(self.promise.get().is_null(), "only one waiter is supported");
        if self.notified.replace(false) {
            // Already notified, don't suspend
            return;
        }

        let promise = unsafe { ffi::emscripten_promise_create() };
        assert!(!promise.is_null());
        self.promise.set(promise);
        // No borrow is held across suspension. Wakeups resolve this handle;
        // additional wakeups before resuming are coalesced by notified.
        unsafe { emscripten_promise_await_unchecked(promise) };
        self.promise.set(null_mut());
        unsafe { ffi::emscripten_promise_destroy(promise) };
        self.notified.set(false);
    }

    fn woken(&self) -> bool {
        self.notified.get()
    }
}

impl Wake for PromiseWaker {
    fn wake(self: Arc<Self>) {
        self.notify();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.notify();
    }
}

impl ArcWake for PromiseWaker {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.notify();
    }
}
