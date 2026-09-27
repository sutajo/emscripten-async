use super::*;
use crate::executor::block_on;
use emscripten_functions_sys::emscripten as ffi;
use futures::StreamExt;
use std::time::Duration;

#[test]
fn sleep_completes() {
    for duration in [Duration::ZERO, Duration::from_millis(100)] {
        block_on(sleep(duration));
    }
}

#[test]
fn fetch_returns_owned_bytes() {
    // The Node runner loads local paths through Emscripten's async loader.
    let bytes = block_on(fetch("Cargo.toml")).unwrap();
    assert_eq!(bytes, include_bytes!("../../Cargo.toml"));
}

#[test]
fn fetch_reports_download_failure() {
    assert!(block_on(fetch("__emscripten_async_missing_fetch_test__")).is_err());
}

#[test]
fn fetch_rejects_nul_in_url() {
    let error = block_on(fetch("invalid\0url")).unwrap_err();
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
        assert!(wget("bad\0url", "/file").await.is_err());
        assert!(wget2_data("/bytes", "INVALID", "").await.is_err());
        assert!(wget2("/bytes", "/file", "INVALID", "").await.is_err());
        assert!(preload_data(b"data", "bad\0suffix").await.is_err());
        assert!(unsafe { load_script("bad\0url").await }.is_err());
    });
}

#[test]
fn missing_preload_file_returns_error() {
    assert_eq!(
        block_on(preload("/__missing_preload_file__"))
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
}
