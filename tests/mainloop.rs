#[cfg(target_os = "emscripten")]
use std::time::Duration;

#[cfg(target_os = "emscripten")]
use emscripten_futures::task::{self, spawn_local};
#[cfg(target_os = "emscripten")]
use futures::StreamExt;

#[cfg(target_os = "emscripten")]
struct AppState {
    counter: usize,
}

#[cfg(target_os = "emscripten")]
fn main() {
    let mut app = AppState { counter: 0 };

    let mut timer = task::interval(Duration::from_millis(100));

    spawn_local(async move {
        loop {
            timer.next().await;
            app.counter += 1;
            println!("Counter: {}", app.counter);

            if app.counter == 50 {
                println!("Exiting");
                break;
            }
        }
    });
}
#[cfg(not(target_os = "emscripten"))]
fn main() {}
