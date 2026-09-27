use futures::task::ArcWake;
use std::{cell::RefCell, sync::Arc, task::Wake};

mod local_pool;
pub use local_pool::*;

#[cfg(test)]
mod tests;

use crate::promise::Promise;

#[derive(Default)]
struct PromiseWaker {
    promise: RefCell<Promise<()>>,
}

unsafe impl Send for PromiseWaker {}
unsafe impl Sync for PromiseWaker {}

impl PromiseWaker {
    fn notify(&self) {
        let _ = self.promise.borrow().fulfill(());
    }

    fn wait(&self) {
        self.promise.borrow().wait();
        // A promise retains its result. Start a fresh notification cycle after
        // releasing both the result borrow and the borrow of the old promise.
        self.promise.replace(Promise::default());
    }

    fn woken(&self) -> bool {
        self.promise.borrow().is_resolved()
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
