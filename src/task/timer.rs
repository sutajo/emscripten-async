use super::{complete, completion, receive, succeeded};
use crate::channel::mpsc;
use emscripten_functions_sys::{emscripten as ffi, html5};
use std::{ffi::c_void, time::Duration};

/// Suspends for the given duration without blocking the event loop.
///
/// The timer starts when the future is first polled. Fractional milliseconds
/// are truncated, and a zero duration yields to the event loop once.
/// Dropping the future leaves the pending timer to clean up when it fires.
///
/// Panics if the duration in whole milliseconds exceeds `i32::MAX`.
pub async fn sleep(duration: Duration) {
    let millis = i32::try_from(duration.as_millis()).expect("sleep duration exceeds i32::MAX ms");
    let (arg, receiver) = completion::<()>();

    // SAFETY: The callback owns the allocation until the timer fires.
    unsafe { ffi::emscripten_async_call(Some(succeeded), arg, millis) };
    receive(receiver).await.expect("sleep callback failed");
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
/// behind, extra ticks are discarded. After dropping the stream, the next
/// scheduled tick stops the timer and releases its callback state.
pub type Ticks = mpsc::Receiver<Duration>;

type TickSender = mpsc::Sender<Duration>;

// Loop callbacks own their sender until the receiving stream is disconnected.
unsafe extern "C" fn loop_tick(time: f64, arg: *mut c_void) -> bool {
    let sender = unsafe { Box::from_raw(arg.cast::<TickSender>()) };
    let keep_running = match sender.send(Duration::from_secs_f64(time / 1000.0)) {
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
    // Preserve the old futures channel's buffer plus its one sender slot.
    let (sender, receiver) = mpsc::bounded(2);
    (Box::into_raw(Box::new(sender)).cast(), receiver)
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

/// Periodic ticks through `emscripten_set_interval`.
/// After dropping the stream, the next tick clears the timer.
pub fn interval(period: Duration) -> Ticks {
    struct Interval {
        sender: TickSender,
        id: i32,
    }

    unsafe extern "C" fn tick(arg: *mut c_void) {
        let interval_ptr = arg.cast::<Interval>();
        // The callback retains the allocation until the receiver disconnects.
        let state = unsafe { &*interval_ptr };

        let time = Duration::from_secs_f64(unsafe { ffi::emscripten_get_now() } / 1000.0);
        match state.sender.send(time) {
            Err(error) if !error.is_full() => {
                unsafe { html5::emscripten_clear_interval(state.id) };
                let _ = state;
                drop(unsafe { Box::from_raw(interval_ptr) });
            }
            _ => {}
        }
    }

    let millis = i32::try_from(period.as_millis()).expect("period exceeds i32::MAX ms");
    let (sender, receiver) = mpsc::bounded(2);
    let state = Box::into_raw(Box::new(Interval { sender, id: 0 }));

    // set_interval schedules its first callback after returning, so initialize
    // the handle before transferring control back to the event loop.
    unsafe {
        (*state).id = html5::emscripten_set_interval(Some(tick), millis as f64, state.cast());
    }
    receiver
}
