use std::{
    cell::{Ref, RefCell},
    ptr::null_mut,
    rc::{Rc, Weak},
};

use emscripten_functions_sys::emscripten::*;

/// A local promise that settles once and retains its result for all waiters.
pub struct Promise<T> {
    // Only active waiters own the handle; the promise can find it to resolve it.
    handle: RefCell<Weak<PromiseHandle>>,
    value: RefCell<Option<Result<T, ()>>>,
}

struct PromiseHandle(em_promise_t);

impl Drop for PromiseHandle {
    fn drop(&mut self) {
        unsafe { emscripten_promise_destroy(self.0) };
    }
}

impl<T> Promise<T> {
    /// Fulfills the promise, returning the value if a result is already resolved.
    pub fn fulfill(&self, value: T) -> Result<(), T> {
        self.try_resolve(Ok(value)).map_err(|err| err.unwrap())
    }

    /// Rejects the promise, returning an error if a result is already resolved.
    pub fn reject(&self) -> Result<(), ()> {
        self.try_resolve(Err(())).map_err(|_| ())
    }

    fn try_resolve(&self, result: Result<T, ()>) -> Result<(), Result<T, ()>> {
        if self.value.borrow().is_none() {
            let fulfilled = result.is_ok();
            *self.value.borrow_mut() = Some(result);

            let handle = self.handle.borrow().upgrade();
            if let Some(handle) = handle {
                unsafe {
                    emscripten_promise_resolve(
                        handle.0,
                        if fulfilled {
                            em_promise_result_t_EM_PROMISE_FULFILL
                        } else {
                            em_promise_result_t_EM_PROMISE_REJECT
                        },
                        null_mut(),
                    );
                }
            }

            Ok(())
        } else {
            Err(result)
        }
    }

    /// Suspends until completion and borrows the retained value.
    ///
    /// Returns `None` on rejection. Repeated and reentrant waits observe the
    /// same result; waiting never consumes the value or resets the promise.
    /// Multiple returned borrows may coexist.
    ///
    /// Pending waits require a JSPI-enabled call context. This method manages
    /// the promise handle, not separate Emscripten stacks for concurrent calls.
    pub fn wait(&self) -> Option<Ref<'_, T>> {
        if !self.is_resolved() {
            let handle = {
                let mut shared = self.handle.borrow_mut();
                shared.upgrade().unwrap_or_else(|| {
                    let raw = unsafe { emscripten_promise_create() };
                    assert!(!raw.is_null());
                    let handle = Rc::new(PromiseHandle(raw));
                    *shared = Rc::downgrade(&handle);
                    handle
                })
            };
            // Release the RefCell borrow before suspending so callbacks and
            // other waiters can access it. The last Rc destroys the handle.
            unsafe { emscripten_promise_await(handle.0) };
        }

        Ref::filter_map(self.value.borrow(), |value| value.as_ref()?.as_ref().ok()).ok()
    }

    pub fn is_resolved(&self) -> bool {
        self.value.borrow().is_some()
    }

    /// Whether a call to [`Self::wait`] is currently suspended.
    pub fn has_waiter(&self) -> bool {
        self.handle.borrow().strong_count() != 0
    }
}

impl<T> Default for Promise<T> {
    fn default() -> Self {
        Self {
            handle: RefCell::default(),
            value: RefCell::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use crate::promise::Promise;

    #[derive(Debug)]
    struct DropCounter(Rc<Cell<usize>>);

    impl Drop for DropCounter {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    #[test]
    fn promise_create_destroy() {
        Promise::<()>::default();
    }

    #[test]
    fn promise_resolve_immediately() {
        let p = Promise::<i32>::default();
        p.fulfill(100).unwrap();
        let first = p.wait().unwrap();
        let second = p.wait().unwrap();
        assert_eq!(*first, 100);
        assert!(std::ptr::eq(&*first, &*second));
        assert!(p.is_resolved());
        assert!(!p.has_waiter());
    }

    #[test]
    fn promise_reject() {
        let p = Promise::<()>::default();
        p.reject().unwrap();
        assert!(p.wait().is_none());
        assert!(p.wait().is_none());
        assert!(p.is_resolved());
    }

    #[test]
    fn borrowed_value_remains_owned_by_the_promise() {
        let drops = Rc::new(Cell::new(0));
        let p = Promise::default();
        p.fulfill(DropCounter(drops.clone())).unwrap();
        let value = p.wait().unwrap();
        assert!(p.is_resolved());
        assert_eq!(drops.get(), 0);
        drop(value);
        assert_eq!(drops.get(), 0);
        drop(p);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn dropping_unwaited_promise_drops_value() {
        let drops = Rc::new(Cell::new(0));
        let p = Promise::default();
        p.fulfill(DropCounter(drops.clone())).unwrap();
        drop(p);
        assert_eq!(drops.get(), 1);
    }

    #[test]
    fn repeated_fulfillment_returns_unaccepted_value() {
        let drops = Rc::new(Cell::new(0));
        let p = Promise::default();
        p.fulfill(DropCounter(drops.clone())).unwrap();
        let borrowed = p.wait().unwrap();
        let unaccepted = p.fulfill(DropCounter(drops.clone())).unwrap_err();
        assert_eq!(p.reject(), Err(()));
        assert_eq!(drops.get(), 0);
        drop(unaccepted);
        assert_eq!(drops.get(), 1);
        drop(borrowed);
        assert_eq!(drops.get(), 1);
        drop(p);
        assert_eq!(drops.get(), 2);
    }

    #[test]
    fn fulfillment_after_rejection_returns_value() {
        let drops = Rc::new(Cell::new(0));
        let p = Promise::default();
        p.reject().unwrap();
        assert_eq!(p.reject(), Err(()));
        let unaccepted = p.fulfill(DropCounter(drops.clone())).unwrap_err();
        assert_eq!(drops.get(), 0);
        drop(unaccepted);
        assert_eq!(drops.get(), 1);
        assert!(p.wait().is_none());
    }

    #[test]
    fn callback_completion_is_retained_for_later_waits() {
        unsafe extern "C" fn fulfill(arg: *mut std::ffi::c_void) {
            let promise = unsafe { Rc::from_raw(arg.cast::<Promise<usize>>()) };
            promise.fulfill(42).unwrap();
        }
        let promise = Rc::new(Promise::<usize>::default());
        let arg = Rc::into_raw(promise.clone()).cast_mut().cast();
        unsafe {
            emscripten_functions_sys::emscripten::emscripten_async_call(Some(fulfill), arg, 0);
        }
        let first = promise.wait().unwrap();
        let second = promise.wait().unwrap();
        assert_eq!(*first, 42);
        assert!(std::ptr::eq(&*first, &*second));
        assert!(promise.is_resolved());
        assert!(!promise.has_waiter());
        assert!(promise.handle.borrow().upgrade().is_none());
    }

    #[test]
    fn callback_can_borrow_the_result_before_the_suspended_waiter_resumes() {
        use emscripten_functions_sys::emscripten as ffi;
        use std::ffi::c_void;

        struct State {
            promise: Promise<usize>,
            reject: bool,
            observed: Cell<Option<usize>>,
            retained_handle: Cell<bool>,
        }

        unsafe extern "C" fn settle(arg: *mut c_void) {
            let state = unsafe { Rc::from_raw(arg.cast::<State>()) };
            let handle = state.promise.handle.borrow().clone();
            if state.reject {
                state.promise.reject().unwrap();
            } else {
                state.promise.fulfill(42).unwrap();
            }
            // Reenter wait while the original caller is still suspended.
            // This settled wait must not destroy that caller's handle.
            let result = state.promise.wait();
            state.observed.set(result.as_deref().copied());
            state.retained_handle.set(
                handle.upgrade().is_some()
                    && handle.ptr_eq(&state.promise.handle.borrow())
                    && state.promise.has_waiter(),
            );
        }

        for reject in [false, true] {
            let state = Rc::new(State {
                promise: Promise::default(),
                reject,
                observed: Cell::new(None),
                retained_handle: Cell::new(false),
            });
            let arg = Rc::into_raw(state.clone()).cast_mut().cast();
            unsafe { ffi::emscripten_async_call(Some(settle), arg, 0) };
            let first = state.promise.wait();
            let expected = if reject { None } else { Some(42) };
            assert_eq!(first.as_deref().copied(), expected);
            assert_eq!(state.observed.get(), expected);
            assert!(state.retained_handle.get());
            assert!(!state.promise.has_waiter());
            assert!(state.promise.handle.borrow().upgrade().is_none());
            assert_eq!(state.promise.wait().as_deref().copied(), expected);
        }
    }
}
