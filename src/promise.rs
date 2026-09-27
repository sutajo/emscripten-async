use std::{
    cell::{Cell, RefCell},
    ptr::null_mut,
};

use emscripten_functions_sys::emscripten::*;

/// A reusable local promise whose payload remains owned by Rust until it is taken.
pub struct Promise<T> {
    handle: Cell<em_promise_t>,
    value: RefCell<Option<Result<T, ()>>>,
    waiting: Cell<bool>,
}

impl<T> Promise<T> {
    /// Fulfills the promise, returning the value if a result is already resolved.
    pub fn fulfill(&self, value: T) -> Result<(), T> {
        let mut value_ref = self.value.borrow_mut();
        if value_ref.is_none() {
            *value_ref = Some(Ok(value));

            if self.waiting.get() {
                unsafe {
                    emscripten_promise_resolve(
                        self.handle.get(),
                        em_promise_result_t_EM_PROMISE_FULFILL,
                        null_mut(),
                    );
                }
            }

            Ok(())
        } else {
            Err(value)
        }
    }

    /// Rejects the promise, returning an error if a result is already resolved.
    pub fn reject(&self) -> Result<(), ()> {
        let mut value_ref = self.value.borrow_mut();
        if value_ref.is_none() {
            *value_ref = Some(Err(()));

            if self.waiting.get() {
                unsafe {
                    emscripten_promise_resolve(
                        self.handle.get(),
                        em_promise_result_t_EM_PROMISE_REJECT,
                        null_mut(),
                    );
                }
            }

            Ok(())
        } else {
            Err(())
        }
    }

    /// Suspends until completion, consumes the result, and resets the promise.
    /// Returns `None` on rejection. A subsequent wait awaits a new completion.
    pub fn wait(&self) -> Option<T> {
        {
            let mut value = self.value.borrow_mut();
            if value.is_some() {
                return value.take().unwrap().ok();
            }
        }

        self.waiting.set(true);
        unsafe { emscripten_promise_await(self.handle.get()) };
        self.waiting.set(false);

        let result = self.value.borrow_mut().take().unwrap();
        // JavaScript promises settle only once; each cycle needs a fresh handle.
        let previous = self.handle.replace(unsafe { emscripten_promise_create() });
        unsafe { emscripten_promise_destroy(previous) };
        result.ok()
    }

    pub fn is_resolved(&self) -> bool {
        self.value.borrow().is_some()
    }
}

impl<T> Default for Promise<T> {
    fn default() -> Self {
        let handle = unsafe { emscripten_promise_create() };
        Self {
            handle: Cell::new(handle),
            value: RefCell::default(),
            waiting: Cell::default(),
        }
    }
}

impl<T> Drop for Promise<T> {
    fn drop(&mut self) {
        unsafe { emscripten_promise_destroy(self.handle.get()) };
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
        assert_eq!(p.wait(), Some(100));
    }

    #[test]
    fn promise_reject() {
        let p = Promise::<()>::default();
        p.reject().unwrap();
        assert_eq!(p.wait(), None);
    }

    #[test]
    fn consumed_value_remains_owned_by_the_caller() {
        let drops = Rc::new(Cell::new(0));
        let p = Promise::default();
        p.fulfill(DropCounter(drops.clone())).unwrap();
        let value = p.wait().unwrap();
        assert!(!p.is_resolved());
        drop(p);
        assert_eq!(drops.get(), 0);
        drop(value);
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
        let unaccepted = p.fulfill(DropCounter(drops.clone())).unwrap_err();
        assert_eq!(p.reject(), Err(()));
        assert_eq!(drops.get(), 0);
        drop(unaccepted);
        assert_eq!(drops.get(), 1);
        drop(p.wait().unwrap());
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
    fn promise_can_wait_for_multiple_callback_completions() {
        unsafe extern "C" fn fulfill(arg: *mut std::ffi::c_void) {
            let promise = unsafe { Rc::from_raw(arg.cast::<Promise<usize>>()) };
            promise.fulfill(42).unwrap();
        }
        let promise = Rc::new(Promise::default());
        for cycle in 0..3 {
            // Exercise both already-completed paths before another pending wait.
            if cycle == 1 {
                promise.fulfill(7).unwrap();
                assert_eq!(promise.wait(), Some(7));
            } else if cycle == 2 {
                promise.reject().unwrap();
                assert_eq!(promise.wait(), None);
            }
            assert!(!promise.is_resolved());
            let arg = Rc::into_raw(promise.clone()).cast_mut().cast();
            unsafe {
                emscripten_functions_sys::emscripten::emscripten_async_call(Some(fulfill), arg, 0);
            }
            assert_eq!(promise.wait(), Some(42));
            assert!(!promise.is_resolved());
        }
    }
}
