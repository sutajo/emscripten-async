use emscripten_functions_sys::emscripten as ffi;
use emscripten_futures::{executor::block_on, task::*};
use futures::{StreamExt, future::Either};
use std::time::Duration;

fn main() {
    println!("running browser_downloads_and_http_errors");
    browser_downloads_and_http_errors();
    println!("running browser_download_cancellation");
    browser_download_cancellation();
    println!("running browser_indexeddb_round_trip");
    browser_indexeddb_round_trip();
    println!("running browser_legacy_callbacks_and_preloading");
    browser_legacy_callbacks_and_preloading();
    println!("running browser_canceled_legacy_request_does_not_steal_next_callback");
    browser_canceled_legacy_request_does_not_steal_next_callback();
    println!("running browser_animation_frames");
    browser_animation_frames();
    println!("running browser_worker_reply_is_owned");
    browser_worker_reply_is_owned();
    println!("browser tests passed");
}

fn browser_downloads_and_http_errors() {
    block_on(async {
        assert_eq!(
            wget2_data("/bytes", "GET", "").await.unwrap(),
            [0, 1, 2, 255]
        );
        assert!(wget2_data("/empty", "GET", "").await.unwrap().is_empty());
        assert_eq!(
            wget2_data("/echo", "POST", "hello=world").await.unwrap(),
            b"hello=world"
        );
        let error = wget2_data("/missing", "GET", "").await.unwrap_err();
        assert!(error.to_string().contains("404"));
        wget2("/bytes", "/download-test.bin", "GET", "")
            .await
            .unwrap();
        assert_eq!(std::fs::read("/download-test.bin").unwrap(), [0, 1, 2, 255]);
        assert!(
            wget2("/missing", "/missing-test.bin", "GET", "")
                .await
                .is_err()
        );
        std::fs::remove_file("/download-test.bin").unwrap();
    });
}

fn browser_download_cancellation() {
    block_on(async {
        let request = Box::pin(wget2_data("/delay", "GET", ""));
        let timer = Box::pin(sleep(Duration::ZERO));
        match futures::future::select(request, timer).await {
            Either::Right(((), request)) => drop(request),
            Either::Left(_) => panic!("delayed download completed before cancellation"),
        }
        sleep(Duration::from_millis(50)).await;
        assert_eq!(
            wget2_data("/bytes", "GET", "").await.unwrap(),
            [0, 1, 2, 255]
        );
    });
}

fn browser_indexeddb_round_trip() {
    block_on(async {
        let db = Idb::new("emscripten-futures-unit-tests").unwrap();
        db.clear().await.unwrap();
        assert!(!db.exists("key").await.unwrap());
        let (first, second) = futures::join!(db.store("key", &[0, 255, 7]), db.store("empty", &[]));
        first.unwrap();
        second.unwrap();
        assert!(db.exists("key").await.unwrap());
        assert_eq!(db.load("key").await.unwrap(), [0, 255, 7]);
        assert!(db.load("empty").await.unwrap().is_empty());
        db.delete("key").await.unwrap();
        assert!(!db.exists("key").await.unwrap());
        assert!(db.load("key").await.is_err());
        db.clear().await.unwrap();
        assert!(!db.exists("empty").await.unwrap());
    });
}

fn browser_legacy_callbacks_and_preloading() {
    block_on(async {
        let (first, second) = futures::join!(
            wget("/bytes", "/legacy-one.bin"),
            wget("/bytes", "/legacy-two.bin")
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(std::fs::read("/legacy-one.bin").unwrap(), [0, 1, 2, 255]);
        assert_eq!(std::fs::read("/legacy-two.bin").unwrap(), [0, 1, 2, 255]);
        preload("/legacy-one.bin").await.unwrap();
        assert!(!preload_data(b"content", "bin").await.unwrap().is_empty());
        assert!(wget("/missing", "/missing-legacy.bin").await.is_err());
        unsafe { load_script("/script.js").await }.unwrap();
        unsafe { load_script("/script.js").await }.unwrap();
        assert_eq!(
            unsafe { ffi::emscripten_run_script_int(c"globalThis.__async_script_loads".as_ptr()) },
            2
        );
        assert!(unsafe { load_script("/missing").await }.is_err());
        std::fs::remove_file("/legacy-one.bin").unwrap();
        std::fs::remove_file("/legacy-two.bin").unwrap();
    });
}

fn browser_canceled_legacy_request_does_not_steal_next_callback() {
    block_on(async {
        let request = Box::pin(wget("/delay", "/abandoned-legacy.bin"));
        match futures::future::select(request, Box::pin(sleep(Duration::ZERO))).await {
            Either::Right(((), request)) => drop(request),
            Either::Left(_) => panic!("delayed download completed before cancellation"),
        }
        wget("/bytes", "/next-legacy.bin").await.unwrap();
        assert_eq!(std::fs::read("/next-legacy.bin").unwrap(), [0, 1, 2, 255]);
        std::fs::remove_file("/abandoned-legacy.bin").unwrap();
        std::fs::remove_file("/next-legacy.bin").unwrap();
    });
}

fn browser_animation_frames() {
    block_on(async {
        let first: Duration = animation_frame().await;
        let now = Duration::from_secs_f64(unsafe { ffi::emscripten_get_now() } / 1000.0);
        assert!(now >= first);
        assert!(now - first < Duration::from_secs(1));
        let mut frames = animation_frames();
        let second: Duration = frames.next().await.unwrap();
        assert!(second >= first);
        drop(frames);
        animation_frame().await;
    });
}

fn browser_worker_reply_is_owned() {
    let worker = unsafe { ffi::emscripten_create_worker(c"/worker.js".as_ptr()) };
    let bytes = block_on(unsafe { call_worker(worker, "echo", &[0, 7, 255]) }).unwrap();
    unsafe { ffi::emscripten_destroy_worker(worker) };
    assert_eq!(bytes, [0, 7, 255]);
}
