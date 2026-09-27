use emscripten_rs_sys::em_asm::{SignatureBuilder, emscripten_asm_const_int, js_asm};
use futures::{FutureExt, future::LocalBoxFuture};
use send_wrapper::SendWrapper;
use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    mem::forget,
    rc::{Rc, Weak},
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

use crate::task::spawn::keepalive::EmscriptenKeepalive;

#[cfg(test)]
mod tests;

mod keepalive {
    use emscripten_rs_sys::{emscripten_runtime_keepalive_pop, emscripten_runtime_keepalive_push};

    pub struct EmscriptenKeepalive {}

    impl Default for EmscriptenKeepalive {
        fn default() -> Self {
            unsafe { emscripten_runtime_keepalive_push() };
            Self {}
        }
    }

    impl Drop for EmscriptenKeepalive {
        fn drop(&mut self) {
            unsafe { emscripten_runtime_keepalive_pop() };
        }
    }
}

struct SpawnedTask {
    future: RefCell<Option<LocalBoxFuture<'static, ()>>>,
    mwaker: Arc<QueueMicrotaskWaker>,
    waker: Waker,
    _keepalive: EmscriptenKeepalive,
}

// Schedules a poll with `queueMicrotask` on wake().
struct QueueMicrotaskWaker {
    state: SendWrapper<MicrotaskWakerState>,
}

struct MicrotaskWakerState {
    task: Weak<SpawnedTask>,
    woken: Cell<bool>,
    pending: Cell<bool>,
}

impl Wake for QueueMicrotaskWaker {
    fn wake(self: Arc<Self>) {
        let state = &*self.state;
        if state.woken.replace(true) {
            // Already woken
            return;
        }

        if let Some(task) = state.task.upgrade() {
            // Only the first wake schedules
            schedule(Rc::into_raw(task));
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn emscripten_futures_poll_task(task: *const c_void) {
    let task_ptr = task.cast::<SpawnedTask>();
    if unsafe { &*task_ptr }.mwaker.state.pending.take() {
        // The previous suspension leaked a reference.
        unsafe { Rc::decrement_strong_count(task_ptr) };
    }

    let task = unsafe { Rc::from_raw(task_ptr) };
    task.mwaker.state.woken.set(false);

    let mut future_borrow = task.future.borrow_mut();
    if let Some(future) = &mut *future_borrow {
        let result = future.poll_unpin(&mut Context::from_waker(&task.waker));

        match result {
            Poll::Ready(()) => {
                let future = future_borrow.take();
                drop(future_borrow);
                // Run user destructors after releasing the mutable borrow.
                drop(future);
            }
            Poll::Pending => {
                task.mwaker.state.pending.set(true);
                drop(future_borrow);

                // Don't drop the task yet, we need to continue later
                forget(task);
            }
        }
    }
}

fn schedule(raw_task: *const SpawnedTask) {
    js_asm! {
        |raw_task| {
            queueMicrotask(() => {
                _emscripten_futures_poll_task(raw_task);
            });
        }
    }
}

/// Spawns a detached future on the calling thread (requires the `spawn` feature).
///
/// The first poll and subsequent wakeups are scheduled with `queueMicrotask`;
/// this function never polls the future inline. No running [`LocalPool`](crate::executor::LocalPool)
/// is required. The future may hold non-`Send` values and keeps the Emscripten
/// runtime alive until it completes. There is no cancellation handle.
///
/// Cross-thread wakeups are not supported and panic before accessing task state.
/// Drop retained wakers on the originating thread: dropping the last waker on
/// another thread panics and skips cleanup of its local state.
/// Await asynchronous operations inside the future rather than calling
/// `block_on` or otherwise suspending a poll with JSPI.
pub fn spawn_local(f: impl Future<Output = ()> + 'static) {
    let spawned_task: Rc<SpawnedTask> = Rc::new_cyclic(|weak| {
        let mwaker = Arc::new(QueueMicrotaskWaker {
            state: SendWrapper::new(MicrotaskWakerState {
                task: weak.clone(),
                woken: Cell::new(false),
                pending: Cell::new(false),
            }),
        });
        SpawnedTask {
            future: Some(f.boxed_local()).into(),
            waker: Waker::from(mwaker.clone()),
            mwaker,
            _keepalive: EmscriptenKeepalive::default(),
        }
    });

    schedule(Rc::into_raw(spawned_task));
}
