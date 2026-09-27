use futures::task::ArcWake;
use std::{sync::Arc, task::Wake};

mod local_pool;
pub use local_pool::*;

#[cfg(test)]
mod tests;

use crate::promise::Promise;

#[derive(Default)]
struct PromiseWaker {
    promise: Promise<()>,
}

unsafe impl Send for PromiseWaker {}
unsafe impl Sync for PromiseWaker {}

impl PromiseWaker {
    fn notify(&self) {
        let _ = self.promise.fulfill(());
    }

    fn wait(&self) {
        self.promise.wait();
    }

    fn woken(&self) -> bool {
        self.promise.is_resolved()
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
