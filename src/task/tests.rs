use super::*;
use crate::executor::block_on;
use emscripten_functions_sys::emscripten as ffi;
use futures::StreamExt;
use std::time::{Duration, Instant};

#[test]
fn completion_can_arrive_before_the_first_poll() {
    let (arg, receiver) = completion::<u32>();
    unsafe { complete(arg, Ok(42)) };
    assert_eq!(block_on(receive(receiver)).unwrap(), 42);
}

#[test]
fn completion_cancellation_releases_callback_values() {
    let (arg, receiver) = completion::<std::rc::Rc<()>>();
    drop(receiver);
    let value = std::rc::Rc::new(());
    unsafe { complete(arg, Ok(value.clone())) };
    assert_eq!(std::rc::Rc::strong_count(&value), 1);

    let (arg, receiver) = completion::<()>();
    // Simulate the callback state being released without producing a result.
    drop(unsafe { Box::from_raw(arg.cast::<Completion<()>>()) });
    assert_eq!(
        block_on(receive(receiver)).unwrap_err().to_string(),
        "operation canceled"
    );
}

#[crate::test]
async fn sleep_completes() {
    for duration in [Duration::ZERO, Duration::from_millis(100)] {
        sleep(duration).await;
    }
}

#[test]
#[allow(deprecated)] // Exercise the retained legacy APIs.
fn legacy_data_returns_owned_bytes() {
    // The Node runner loads local paths through Emscripten's async loader.
    let bytes = block_on(Wget::legacy_data("Cargo.toml")).unwrap();
    assert_eq!(bytes, include_bytes!("../../Cargo.toml"));
}

#[test]
#[allow(deprecated)] // Exercise the retained legacy APIs.
fn legacy_data_reports_download_failure() {
    assert!(block_on(Wget::legacy_data("__emscripten_async_missing_fetch_test__")).is_err());
}

#[test]
#[allow(deprecated)] // Exercise the retained legacy APIs.
fn legacy_data_rejects_nul_in_url() {
    let error = block_on(Wget::legacy_data("invalid\0url")).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn timer_operations_complete() {
    block_on(async {
        yield_now().await;
        timeout(Duration::ZERO).await;
        timeout(Duration::from_millis(2)).await;
        for mut ticks in [
            interval(Duration::from_millis(2)),
            timeout_loop(Duration::from_millis(2)),
            immediate_loop(),
        ] {
            let first: Duration = ticks.next().await.unwrap();
            let second: Duration = ticks.next().await.unwrap();
            assert!(second >= first);
            let now = Duration::from_secs_f64(unsafe { ffi::emscripten_get_now() } / 1000.0);
            assert!(now >= second);
            assert!(now - second < Duration::from_secs(1));
            drop(ticks);
        }
        // Let callback-driven loops observe that their receivers were dropped.
        sleep(Duration::from_millis(5)).await;
    });
}

#[crate::test]
async fn interval_each_tick_is_within_20ms() {
    let period = Duration::from_millis(120);
    let tolerance = Duration::from_millis(20);
    let mut previous = Instant::now();
    let mut ticks = interval(period);
    for tick in 1..=5 {
        ticks
            .next()
            .await
            .expect("interval ended before five ticks");
        let now = Instant::now();
        let elapsed = now.duration_since(previous);
        previous = now;
        assert!(
            elapsed.abs_diff(period) <= tolerance,
            "tick {tick} arrived after {elapsed:?}; expected {period:?} ± {tolerance:?}"
        );
    }
}

#[test]
fn main_loop_processes_both_blocker_types() {
    unsafe extern "C" fn frame() {}
    unsafe {
        ffi::emscripten_set_main_loop_expected_blockers(1);
        ffi::emscripten_set_main_loop(Some(frame), 60, false);
    }
    block_on(async {
        futures::join!(main_loop_blocker(true), main_loop_blocker(false));
    });
    unsafe { ffi::emscripten_cancel_main_loop() };
}

#[test]
#[allow(deprecated)] // Exercise the retained legacy APIs.
fn invalid_inputs_fail_before_starting_operations() {
    block_on(async {
        assert_eq!(
            Idb::new("bad\0db").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        let db = Idb::new("db").unwrap();
        assert!(db.load("bad\0key").await.is_err());
        assert!(db.store("bad\0key", b"data").await.is_err());
        assert!(db.delete("bad\0key").await.is_err());
        assert!(db.exists("bad\0key").await.is_err());
        assert!(Wget::legacy_file("bad\0url", "/file").await.is_err());
        assert!(Wget::data("/bytes", "INVALID", "").await.is_err());
        assert!(Wget::file("/bytes", "/file", "INVALID", "").await.is_err());
        assert!(preload_data(b"data", "bad\0suffix").await.is_err());
        assert!(unsafe { load_script("bad\0url").await }.is_err());
    });
}

#[test]
#[allow(deprecated)] // Exercise the retained legacy APIs.
fn missing_preload_file_returns_error() {
    assert_eq!(
        block_on(preload("/__missing_preload_file__"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
}
