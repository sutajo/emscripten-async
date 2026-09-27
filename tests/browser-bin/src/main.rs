use emscripten_futures::{executor::block_on, task::*};
use emscripten_rs_sys as ffi;
use futures::{StreamExt, future::Either};
use std::{cell::Cell, time::Duration};

fn main() {
    println!("running browser_spawned_tasks");
    browser_spawned_tasks();
    println!("running browser_downloads_and_http_errors");
    browser_downloads_and_http_errors();
    println!("running browser_five_file_downloads_with_join");
    browser_five_file_downloads_with_join();
    println!("running browser_download_cancellation");
    browser_download_cancellation();
    println!("running browser_download_progress");
    browser_download_progress();
    println!("running browser_progress_cancellation");
    browser_progress_cancellation();
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
            Wget::data("/bytes", "GET", "").await.unwrap(),
            [0, 1, 2, 255]
        );
        assert!(Wget::data("/empty", "GET", "").await.unwrap().is_empty());
        assert_eq!(
            Wget::data("/echo", "POST", "hello=world").await.unwrap(),
            b"hello=world"
        );
        let error = Wget::data("/missing", "GET", "").await.unwrap_err();
        assert!(error.to_string().contains("404"));
        Wget::file("/bytes", "/download-test.bin", "GET", "")
            .await
            .unwrap();
        assert_eq!(std::fs::read("/download-test.bin").unwrap(), [0, 1, 2, 255]);
        assert!(
            Wget::file("/missing", "/missing-test.bin", "GET", "")
                .await
                .is_err()
        );
        std::fs::remove_file("/download-test.bin").unwrap();
    });
}

fn browser_five_file_downloads_with_join() {
    block_on(async {
        let (a, b, c, d, e) = futures::join!(
            Wget::file("/join-download/0", "/join-0.bin", "GET", ""),
            Wget::file("/join-download/1", "/join-1.bin", "GET", ""),
            Wget::file("/join-download/2", "/join-2.bin", "GET", ""),
            Wget::file("/join-download/3", "/join-3.bin", "GET", ""),
            Wget::file("/join-download/4", "/join-4.bin", "GET", ""),
        );
        for (index, result) in [a, b, c, d, e].into_iter().enumerate() {
            result.unwrap_or_else(|error| panic!("download {index} failed: {error}"));
            let file = format!("/join-{index}.bin");
            assert_eq!(std::fs::read(&file).unwrap(), [index as u8, 0, 255]);
            std::fs::remove_file(file).unwrap();
        }
    });
}

fn browser_download_cancellation() {
    block_on(async {
        let request = Box::pin(Wget::data("/delay", "GET", ""));
        let timer = Box::pin(sleep(Duration::ZERO));
        match futures::future::select(request, timer).await {
            Either::Right(((), request)) => drop(request),
            Either::Left(_) => panic!("delayed download completed before cancellation"),
        }
        sleep(Duration::from_millis(50)).await;
        assert_eq!(
            Wget::data("/bytes", "GET", "").await.unwrap(),
            [0, 1, 2, 255]
        );
    });
}

fn browser_download_progress() {
    block_on(async {
        for (url, total) in [("/progress", Some(65536)), ("/progress-unknown", None)] {
            let mut updates = Vec::new();
            let bytes = Wget::data_with_progress(url, "GET", "", |progress| updates.push(progress))
                .await
                .unwrap();
            assert_eq!(bytes, vec![0xa5; 65536]);
            assert_eq!(
                updates.last(),
                Some(&Progress {
                    loaded: 65536,
                    total
                })
            );
            assert!(updates.iter().all(|progress| progress.total == total));
            assert!(
                updates
                    .windows(2)
                    .all(|pair| pair[0].loaded <= pair[1].loaded)
            );
        }

        let mut percentages = Vec::new();
        Wget::file_with_progress("/progress", "/progress.bin", "GET", "", |percent| {
            percentages.push(percent);
        })
        .await
        .unwrap();
        assert_eq!(percentages.last(), Some(&100));
        assert!(
            percentages
                .iter()
                .all(|percent| (0..=100).contains(percent))
        );
        assert!(percentages.windows(2).all(|pair| pair[0] <= pair[1]));
        assert_eq!(std::fs::read("/progress.bin").unwrap(), vec![0xa5; 65536]);
        std::fs::remove_file("/progress.bin").unwrap();
    });
}

async fn cancel_after_progress(
    request: impl std::future::Future<Output = std::io::Result<()>>,
    calls: &Cell<usize>,
) {
    let first_progress = async {
        while calls.get() == 0 {
            sleep(Duration::from_millis(1)).await;
        }
    };
    match futures::future::select(Box::pin(request), Box::pin(first_progress)).await {
        Either::Right(((), request)) => drop(request),
        Either::Left(_) => panic!("download finished before cancellation after progress"),
    }
    let before = calls.get();
    assert!(before > 0);
    sleep(Duration::from_millis(200)).await;
    assert_eq!(calls.get(), before, "progress continued after cancellation");
}

fn browser_progress_cancellation() {
    block_on(async {
        let calls = Cell::new(0);
        cancel_after_progress(
            async {
                Wget::data_with_progress("/progress", "GET", "", |_| {
                    calls.set(calls.get() + 1);
                })
                .await
                .map(|_| ())
            },
            &calls,
        )
        .await;

        calls.set(0);
        cancel_after_progress(
            Wget::file_with_progress("/progress", "/canceled-progress.bin", "GET", "", |_| {
                calls.set(calls.get() + 1);
            }),
            &calls,
        )
        .await;
        assert!(!std::path::Path::new("/canceled-progress.bin").exists());
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

#[allow(deprecated)] // Exercise the retained legacy APIs.
fn browser_legacy_callbacks_and_preloading() {
    block_on(async {
        let (first, second) = futures::join!(
            Wget::legacy_file("/bytes", "/legacy-one.bin"),
            Wget::legacy_file("/bytes", "/legacy-two.bin")
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(std::fs::read("/legacy-one.bin").unwrap(), [0, 1, 2, 255]);
        assert_eq!(std::fs::read("/legacy-two.bin").unwrap(), [0, 1, 2, 255]);
        preload("/legacy-one.bin").await.unwrap();
        assert!(!preload_data(b"content", "bin").await.unwrap().is_empty());
        assert!(
            Wget::legacy_file("/missing", "/missing-legacy.bin")
                .await
                .is_err()
        );
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

#[allow(deprecated)] // Exercise the retained legacy APIs.
fn browser_canceled_legacy_request_does_not_steal_next_callback() {
    block_on(async {
        let request = Box::pin(Wget::legacy_file("/delay", "/abandoned-legacy.bin"));
        match futures::future::select(request, Box::pin(sleep(Duration::ZERO))).await {
            Either::Right(((), request)) => drop(request),
            Either::Left(_) => panic!("delayed download completed before cancellation"),
        }
        Wget::legacy_file("/bytes", "/next-legacy.bin")
            .await
            .unwrap();
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

fn browser_spawned_tasks() {
    block_on(async {
        let (first_send, first_recv) = futures::channel::oneshot::channel();
        let (second_send, second_recv) = futures::channel::oneshot::channel();
        spawn_local(async move {
            let data = Wget::data("/bytes", "GET", "").await.unwrap();
            first_send.send(data).unwrap();
        });
        spawn_local(async move {
            animation_frame().await;
            sleep(Duration::ZERO).await;
            second_send.send(42).unwrap();
        });
        let (first, second) = futures::join!(first_recv, second_recv);
        assert_eq!(first.unwrap(), [0, 1, 2, 255]);
        assert_eq!(second.unwrap(), 42);
    });
}
