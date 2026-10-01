use emscripten_rs_sys::em_asm::{SignatureBuilder, emscripten_asm_const_int, js_asm};
use futures::{FutureExt, future::LocalBoxFuture, task::waker};
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    task::{Context, Poll, Waker},
};

use crate::{
    send_wrapper::SendWrapper,
    task::spawn::{keepalive::EmscriptenKeepalive, microtask_waker::MicroTaskWaker},
};

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
    inner_waker: Arc<SendWrapper<microtask_waker::MicroTaskWaker>>,
    waker: Waker,
    _keepalive: EmscriptenKeepalive,
}

mod microtask_waker {
    use crate::{
        send_wrapper::SendWrapper,
        task::spawn::{SpawnedTask, schedule},
    };
    use futures::task::ArcWake;
    use std::{cell::Cell, sync::Arc};

    #[repr(u8)]
    #[derive(Clone, Copy, PartialEq, Eq, Default)]
    enum MicroTaskState {
        #[default]
        Sleeping,
        Polled,
        NeedsScheduling,
    }

    pub struct MicroTaskWaker {
        task: *mut SpawnedTask,
        task_state: Cell<MicroTaskState>,
    }

    impl MicroTaskWaker {
        #[inline]
        pub(super) fn new(task: *mut SpawnedTask) -> Self {
            Self {
                task,
                task_state: Cell::default()
            }
        }

        #[inline]
        pub fn before_poll(&self) {
            self.task_state.set(MicroTaskState::Polled);
        }

        #[inline]
        pub fn try_wake(&self) {
            // Only schedule if the task was Sleeping.
            // If the task is being polled currently, it will reschedule itself.
            if self.task_state.replace(MicroTaskState::NeedsScheduling) == MicroTaskState::Sleeping
            {
                schedule(self.task);
            }
        }

        #[inline]
        pub fn try_sleep(&self) {
            // Go back to sleep, but also reschedule the task if during polling somebody woke us.
            if self.task_state.replace(MicroTaskState::Sleeping) == MicroTaskState::NeedsScheduling
            {
                schedule(self.task);
            }
        }
    }

    impl ArcWake for SendWrapper<MicroTaskWaker> {
        fn wake_by_ref(arc_self: &Arc<Self>) {
            arc_self.as_ref().as_ref().try_wake();
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn emscripten_futures_poll_task(task: *mut c_void) {
    if let Err(payload) = catch_unwind(AssertUnwindSafe(|| {
        let task_ptr = task.cast::<SpawnedTask>();
        // SAFETY: at most one callback is queued, and the Box stays allocated
        // until its final poll. Wakes during polling cannot queue a callback.
        // Owning the Box inside the catch also drops the task if polling unwinds.
        let mut task = unsafe { Box::from_raw(task_ptr) };
        let waker = task.inner_waker.as_ref().as_ref();

        waker.before_poll();
        let result = task
            .future
            .poll_unpin(&mut Context::from_waker(&task.waker));

        match result {
            Poll::Ready(()) => {
                // Leave SLEEPING clear so retained wakers cannot schedule the
                // freed task, including wakes from the future's destructor.
                drop(task);
            }
            Poll::Pending => {
                waker.try_sleep();
                let _ = Box::into_raw(task);
            }
        }
    })) {
        drop_panic_payload(payload);
    }
}

#[cold]
#[inline(never)]
fn drop_panic_payload(payload: Box<dyn std::any::Any + Send>) {
    // Keep payload destruction and its unwind cleanup off the normal poll path.
    drop(payload);
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
///
/// # Panics
///
/// With `panic = "unwind"`, one catch covers polling and dropping the future.
/// If polling panics, the task is dropped while unwinding, releasing its runtime
/// keepalive. A destructor panic after normal completion is also caught.
/// The panic hook still runs, and other tasks can continue. Retained wakers
/// cannot reschedule the failed task.
/// If the destructor panics during a polling panic's unwind, the double panic
/// aborts. Panics while dropping a caught panic payload are not caught, and
/// `panic = "abort"` builds cannot recover from panics.
///
/// To receive a result or propagate a polling panic to an awaiter, use
/// [`FutureExt::remote_handle`] and spawn its `Remote` future. Keep the handle on
/// the originating thread: dropping it requests cancellation by waking the task.
/// Calling the handle's `forget` method instead lets the task continue detached.
pub fn spawn_local(f: impl Future<Output = ()> + 'static) {
    spawn_local_boxed(f.boxed_local());
}

fn spawn_local_boxed(f: LocalBoxFuture<'static, ()>) {
    let mut uninitialized_task = Box::new_uninit();
    let microtask_waker = MicroTaskWaker::new(uninitialized_task.as_mut_ptr() as _);
    let inner_waker = Arc::new(SendWrapper::new(microtask_waker));
    let waker = waker(inner_waker.clone());
    uninitialized_task.write(SpawnedTask {
        future: f,
        inner_waker,
        waker,
        _keepalive: Default::default(),
    });
    let task = unsafe { uninitialized_task.assume_init() };
    schedule(Box::into_raw(task));
}