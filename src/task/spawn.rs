use emscripten_rs_sys::em_asm::{SignatureBuilder, emscripten_asm_const_int, js_asm};
use futures::{
    FutureExt,
    future::LocalBoxFuture,
    task::{ArcWake, waker_ref},
};
use send_wrapper::SendWrapper;
use std::{
    ffi::c_void,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    task::{Context, Poll},
};

use crate::task::spawn::{keepalive::EmscriptenKeepalive, microtask_waker::MicroTaskWaker};

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
    mwaker: Arc<MicroTaskWakerWrapper>,
    _keepalive: EmscriptenKeepalive,
}

// Records wakes during polling; schedules sleeping tasks with queueMicrotask.
struct MicroTaskWakerWrapper {
    wrapper: SendWrapper<microtask_waker::MicroTaskWaker>,
}

impl MicroTaskWakerWrapper {
    // Share SendWrapper's thread check between polling, waking, and setup.
    #[inline(never)]
    fn waker(&self) -> &MicroTaskWaker {
        &self.wrapper
    }
}

mod microtask_waker {
    use crate::task::spawn::{SpawnedTask, schedule};
    use bitflags::bitflags;
    use std::cell::Cell;

    bitflags! {
        #[derive(Clone, Copy, PartialEq, Eq)]
        struct TaskStateFlags : u8 {
            const NOTIFIED = 1 << 0;
            const SLEEPING = 1 << 1;
        }
    }

    pub struct MicroTaskWaker {
        task: Cell<*mut SpawnedTask>,
        task_state: Cell<TaskStateFlags>,
    }

    impl Default for MicroTaskWaker {
        fn default() -> Self {
            Self {
                task: Default::default(),
                task_state: Cell::new(TaskStateFlags::all()),
            }
        }
    }

    impl MicroTaskWaker {
        #[inline]
        pub fn init(&self, task: *mut SpawnedTask) {
            self.task.replace(task);
        }

        #[inline]
        pub fn start_poll(&self) {
            self.task_state.set(TaskStateFlags::empty());
        }

        #[inline]
        pub fn try_wake(&self) {
            self.update_state::<{ TaskStateFlags::NOTIFIED.bits() }, { TaskStateFlags::SLEEPING.bits() }>();
        }

        #[inline]
        pub fn try_sleep(&self) {
            self.update_state::<{ TaskStateFlags::SLEEPING.bits() }, { TaskStateFlags::NOTIFIED.bits() }>();
        }

        #[inline]
        fn update_state<const SET: u8, const SCHEDULE_IF: u8>(&self) {
            let current_state = self.task_state.get().bits();
            self.task_state
                .set(TaskStateFlags::from_bits_retain(current_state | SET));
            // Decide whether to schedule from the state before setting the flag.
            if current_state == SCHEDULE_IF {
                schedule(self.task.get());
            }
        }
    }
}

impl ArcWake for MicroTaskWakerWrapper {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.waker().try_wake();
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
        let state = task.mwaker.waker();

        state.start_poll();
        let result = {
            let waker = waker_ref(&task.mwaker);
            let mut cx = Context::from_waker(&waker);
            task.future.poll_unpin(&mut cx)
        };

        match result {
            Poll::Ready(()) => {
                // Leave SLEEPING clear so retained wakers cannot schedule the
                // freed task, including wakes from the future's destructor.
                drop(task);
            }
            Poll::Pending => {
                state.try_sleep();
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

// Share the JavaScript call setup between spawning and waking tasks.
#[inline(never)]
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
    let waker_state = SendWrapper::new(MicroTaskWaker::default());
    let mwaker = Arc::new(MicroTaskWakerWrapper {
        wrapper: waker_state,
    });
    let mut spawned_task: Box<SpawnedTask> = Box::new(SpawnedTask {
        future: f,
        mwaker,
        _keepalive: Default::default(),
    });
    let task_ptr = Box::as_mut_ptr(&mut spawned_task);
    spawned_task.mwaker.waker().init(task_ptr);
    schedule(Box::into_raw(spawned_task));
}
