cfg_select! {
    target_os = "emscripten" => {
        use std::time::Duration;
        use emscripten_futures::task::{self, spawn_local};
        use futures::StreamExt;

        struct AppState {
            counter: usize,
        }

        fn main() {
            let mut app = AppState { counter: 0 };

            let mut slow_timer = task::interval(Duration::from_millis(100)).fuse();
            let mut fast_timer = task::interval(Duration::from_millis(20)).fuse();

            spawn_local(async move {
                loop {
                    futures::select! {
                        _ = slow_timer.next() => {
                            println!("Slow");
                        }
                        _ = fast_timer.next() => {
                            println!("Fast");
                        }
                    }

                    app.counter += 1;
                    println!("Counter: {}", app.counter);

                    if app.counter == 50 {
                        println!("Exiting");
                        break;
                    }
                }
            });
        }
    }
    _ => {
        fn main() {}
    }
}
