use emscripten_rs_sys::em_asm::{SignatureBuilder, emscripten_asm_const_int, js_asm};
use futures::{FutureExt, future::LocalBoxFuture};
use send_wrapper::SendWrapper;
use std::{
    cell::Cell,
    ffi::c_void,
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
    future: LocalBoxFuture<'static, ()>,
    mwaker: Arc<QueueMicrotaskWaker>,
    waker: Waker,
    _keepalive: EmscriptenKeepalive,
}

// Records wakes during polling; schedules sleeping tasks with queueMicrotask.
struct QueueMicrotaskWaker {
    state: SendWrapper<MicrotaskWakerState>,
}

struct MicrotaskWakerState {
    task: Cell<*mut SpawnedTask>,
    needs_poll: Cell<bool>,
    // False while polling and after completion, when task may be dangling.
    is_sleeping: Cell<bool>,
}

impl Wake for QueueMicrotaskWaker {
    fn wake(self: Arc<Self>) {
        let state = &*self.state;
        if !state.needs_poll.replace(true) && state.is_sleeping.get() {
            schedule(state.task.get());
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn emscripten_futures_poll_task(task: *mut c_void) {
    let task_ptr = task.cast::<SpawnedTask>();
    // SAFETY: at most one callback is queued, and the Box stays allocated
    // until its final poll. Wakes during polling cannot queue a callback.
    let mut task = unsafe { Box::from_raw(task_ptr) };
    let state = &*task.mwaker.state;

    state.is_sleeping.set(false);
    state.needs_poll.set(false);
    let result = task
        .future
        .poll_unpin(&mut Context::from_waker(&task.waker));

    match result {
        Poll::Ready(()) => {
            // Leave is_sleeping false so retained wakers cannot schedule the
            // freed task, including wakes from the future's destructor.
            drop(task);
        }
        Poll::Pending => {
            state.is_sleeping.set(true);
            let needs_poll = state.needs_poll.get();
            let task_ptr = Box::into_raw(task);
            if needs_poll {
                schedule(task_ptr);
            }
        }
    }
}

fn schedule(raw_task: *mut SpawnedTask) {
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
/// The first poll is deferred to `queueMicrotask`. Each callback polls once;
/// repeated wakes are coalesced. A wake during polling schedules another
/// microtask only if the future returns `Pending`. Wakes after completion on
/// the originating thread are ignored. No running [`LocalPool`](crate::executor::LocalPool)
/// is required. The future may hold non-`Send` values and keeps the Emscripten
/// runtime alive until it completes. There is no cancellation handle.
///
/// Cross-thread wakeups are not supported and panic before accessing task state.
/// Wakers may be cloned and dropped on any thread, but must only be woken on
/// the originating thread.
/// Await asynchronous operations inside the future rather than calling
/// `block_on` or otherwise suspending a poll with JSPI.
pub fn spawn_local(f: impl Future<Output = ()> + 'static) {
    let waker_state = SendWrapper::new(MicrotaskWakerState {
        task: Cell::default(),
        // The first poll is already scheduled, so further wakes must be coalesced.
        needs_poll: Cell::new(true),
        is_sleeping: Cell::new(true),
    });
    let mwaker = Arc::new(QueueMicrotaskWaker { state: waker_state });
    let waker = Waker::from(mwaker.clone());
    let mut spawned_task: Box<SpawnedTask> = Box::new(SpawnedTask {
        future: f.boxed_local(),
        mwaker,
        waker,
        _keepalive: Default::default(),
    });
    let task_ptr = Box::as_mut_ptr(&mut spawned_task);
    spawned_task.mwaker.state.task.set(task_ptr);
    schedule(Box::into_raw(spawned_task));
}
