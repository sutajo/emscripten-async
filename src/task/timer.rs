use super::{complete, completion, receive, succeeded};
use emscripten_functions_sys::{emscripten as ffi, html5};
use futures::channel::oneshot;
use std::{ffi::c_void, time::Duration};

/// Suspends for the given duration without blocking the event loop.
///
/// The timer starts when the future is first polled. Fractional milliseconds
/// are truncated, and a zero duration yields to the event loop once.
/// Dropping the future leaves the pending timer to clean up when it fires.
///
/// Panics if the duration in whole milliseconds exceeds `i32::MAX`.
pub async fn sleep(duration: Duration) {
    unsafe extern "C" fn complete(arg: *mut c_void) {
        // SAFETY: Each scheduled callback receives a unique boxed sender and
        // runs once, taking back ownership even if the receiver was dropped.
        let sender = unsafe { Box::from_raw(arg.cast::<oneshot::Sender<()>>()) };
        let _ = sender.send(());
    }

    let millis = i32::try_from(duration.as_millis()).expect("sleep duration exceeds i32::MAX ms");
    let (sender, receiver) = oneshot::channel::<()>();
    let arg = Box::into_raw(Box::new(sender)).cast::<c_void>();

    // SAFETY: The callback owns the allocation until the timer fires.
    unsafe { ffi::emscripten_async_call(Some(complete), arg, millis) };
    receiver.await.expect("sleep callback dropped its sender");
}

/// Yields through `emscripten_set_immediate`.
pub async fn yield_now() {
    let (arg, receiver) = completion::<()>();
    unsafe { html5::emscripten_set_immediate(Some(succeeded), arg) };
    receive(receiver).await.expect("immediate callback failed");
}

/// Waits using `emscripten_set_timeout` (whole milliseconds).
/// Panics if the delay exceeds the browser timer's signed 32-bit limit.
pub async fn timeout(duration: Duration) {
    let millis = i32::try_from(duration.as_millis()).expect("timeout exceeds i32::MAX ms");
    let (arg, receiver) = completion::<()>();
    unsafe { html5::emscripten_set_timeout(Some(succeeded), millis as f64, arg) };
    receive(receiver).await.expect("timeout callback failed");
}

/// Waits for the next browser animation frame.
/// Returns its timestamp relative to the browser's time origin.
pub async fn animation_frame() -> Duration {
    unsafe extern "C" fn frame(time: f64, arg: *mut c_void) -> bool {
        unsafe { complete(arg, Ok(time)) };
        false
    }
    let (arg, receiver) = completion::<f64>();
    unsafe { html5::emscripten_request_animation_frame(Some(frame), arg) };
    let millis = receive(receiver)
        .await
        .expect("animation frame callback failed");
    Duration::from_secs_f64(millis / 1000.0)
}

/// Waits for a blocker to be processed by an already-running Emscripten main loop.
/// `counted` selects whether it contributes to main-loop loading progress.
pub async fn main_loop_blocker(counted: bool) {
    let (arg, receiver) = completion::<()>();
    unsafe {
        if counted {
            ffi::_emscripten_push_main_loop_blocker(Some(succeeded), arg, c"Rust task".as_ptr());
        } else {
            ffi::_emscripten_push_uncounted_main_loop_blocker(
                Some(succeeded),
                arg,
                c"Rust task".as_ptr(),
            );
        }
    }
    receive(receiver)
        .await
        .expect("main-loop blocker callback failed");
}

/// A bounded stream of timestamps relative to the runtime's time origin.
/// Timestamps preserve fractional milliseconds. If a consumer falls
/// behind, extra ticks are discarded. Dropping stops an interval immediately;
/// callback-driven loops stop at their next scheduled tick.
pub struct Ticks {
    receiver: futures::channel::mpsc::Receiver<f64>,
    interval: Option<i32>,
    // Keeps set_interval user data alive. Each callback clones it before waking.
    _state: Option<std::rc::Rc<std::cell::RefCell<futures::channel::mpsc::Sender<f64>>>>,
}

impl futures::Stream for Ticks {
    type Item = Duration;
    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Duration>> {
        std::pin::Pin::new(&mut self.receiver)
            .poll_next(cx)
            .map(|tick| tick.map(|millis| Duration::from_secs_f64(millis / 1000.0)))
    }
}

impl Drop for Ticks {
    fn drop(&mut self) {
        if let Some(id) = self.interval {
            unsafe { html5::emscripten_clear_interval(id) };
        }
    }
}

type TickSender = futures::channel::mpsc::Sender<f64>;

// Loop callbacks own their sender until the receiving stream is disconnected.
unsafe extern "C" fn loop_tick(time: f64, arg: *mut c_void) -> bool {
    let mut sender = unsafe { Box::from_raw(arg.cast::<TickSender>()) };
    let keep_running = match sender.try_send(time) {
        Ok(()) => true,
        Err(error) => error.is_full(),
    };
    if keep_running {
        let _ = Box::into_raw(sender);
    }
    keep_running
}

unsafe extern "C" fn immediate_tick(arg: *mut c_void) -> bool {
    unsafe { loop_tick(ffi::emscripten_get_now(), arg) }
}

fn tick_channel() -> (*mut c_void, Ticks) {
    let (sender, receiver) = futures::channel::mpsc::channel(1);
    (
        Box::into_raw(Box::new(sender)).cast(),
        Ticks {
            receiver,
            interval: None,
            _state: None,
        },
    )
}

/// Repeated callbacks through `emscripten_set_timeout_loop`.
/// The first tick is immediate; subsequent ticks use the given period.
pub fn timeout_loop(period: Duration) -> Ticks {
    let millis = i32::try_from(period.as_millis()).expect("period exceeds i32::MAX ms");
    let (arg, ticks) = tick_channel();
    unsafe { html5::emscripten_set_timeout_loop(Some(loop_tick), millis as f64, arg) };
    ticks
}

/// Repeated event-loop turns through `emscripten_set_immediate_loop`.
pub fn immediate_loop() -> Ticks {
    let (arg, ticks) = tick_channel();
    unsafe { html5::emscripten_set_immediate_loop(Some(immediate_tick), arg) };
    ticks
}

/// Browser animation frames through `emscripten_request_animation_frame_loop`.
pub fn animation_frames() -> Ticks {
    let (arg, ticks) = tick_channel();
    unsafe { html5::emscripten_request_animation_frame_loop(Some(loop_tick), arg) };
    ticks
}

/// Periodic ticks through `emscripten_set_interval`. Dropping clears the timer.
pub fn interval(period: Duration) -> Ticks {
    unsafe extern "C" fn tick(arg: *mut c_void) {
        let ptr = arg.cast::<std::cell::RefCell<TickSender>>();
        // Keep the state alive if waking the receiver drops the stream inline.
        unsafe { std::rc::Rc::increment_strong_count(ptr) };
        let state = unsafe { std::rc::Rc::from_raw(ptr) };
        let _ = state
            .borrow_mut()
            .try_send(unsafe { ffi::emscripten_get_now() });
    }
    let millis = i32::try_from(period.as_millis()).expect("period exceeds i32::MAX ms");
    let (sender, receiver) = futures::channel::mpsc::channel(1);
    let state = std::rc::Rc::new(std::cell::RefCell::new(sender));
    let arg = std::rc::Rc::as_ptr(&state).cast_mut().cast();
    let id = unsafe { html5::emscripten_set_interval(Some(tick), millis as f64, arg) };
    Ticks {
        receiver,
        interval: Some(id),
        _state: Some(state),
    }
}
