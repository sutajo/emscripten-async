# emscripten-futures

A local async executor and awaitable Emscripten operations for Rust applications
targeting `wasm32-unknown-emscripten`.

The executor uses JavaScript Promise Integration (JSPI) to yield to the browser
or Node event loop while Rust futures are pending. It provides `block_on`,
`LocalPool`, and `LocalSpawner` and works with `futures` combinators such as
`join` and `select`. Run the executor and its wakeups on the same thread.
Cross-thread wakeups panic before accessing executor state.
Its internal notifier coalesces wakeups and creates a JavaScript promise only
when it must suspend; already pending notifications require no allocation.

## Usage

Install the Emscripten SDK and the Rust target:

```text
rustup toolchain install nightly
rustup target add wasm32-unknown-emscripten --toolchain nightly
```

Add the dependency to your application:

```toml
[dependencies]
emscripten-futures = "0.7"
```

The default `spawn` feature requires nightly Rust. Build with `cargo +nightly`.
For stable Rust without spawning, use:

```toml
emscripten-futures = { version = "0.7", default-features = false }
```

Enable JSPI when linking your application. For example, in `.cargo/config.toml`:

```toml
[build]
target = "wasm32-unknown-emscripten"

[target.wasm32-unknown-emscripten]
rustflags = ["-C", "link-arg=-sJSPI"]
runner = "node"
```

Use a browser or Node version with JSPI support. The application's final link
needs `-sJSPI`; this crate's build-script link argument applies to its own targets.

```rust
use emscripten_futures::{executor::block_on, task};
use std::time::Duration;

fn main() {
    block_on(async {
        task::sleep(Duration::from_millis(100)).await;
        println!("timer completed");
    });
}
```

## Task APIs

The `spawn` feature, enabled by default, provides `task::spawn_local`:

```toml
emscripten-futures = { version = "0.7", features = ["spawn"] }
```

The `spawn` feature enables `emscripten_rs_sys/nightly` and requires nightly Rust.
It is enabled by default; use `default-features = false` for the stable-compatible
API without spawning. Spawning uses `EM_ASM` to schedule polling with JavaScript's
`queueMicrotask`. Spawned futures run on the calling thread, may hold non-`Send`
values, and do not require a running `LocalPool`. The first poll is deferred;
each task keeps the runtime alive until it completes. Cross-thread wakeups are
not supported and panic before accessing task state. Drop retained wakers on
the originating thread: dropping the last waker on another thread panics and
skips cleanup of its local state. Tasks are detached and have no cancellation handle.

```rust,ignore
emscripten_futures::task::spawn_local(async {
    emscripten_futures::task::sleep(std::time::Duration::from_millis(100)).await;
    println!("background task completed");
});
```

- Timers, animation frames, and streams of `Duration` timestamps.
- Downloads to owned bytes or the Emscripten virtual filesystem.
- `Idb` for asynchronous IndexedDB storage.
- Script loading, asset preloading, main-loop blockers, worker replies, and
  dynamic-library loading.

Browser APIs such as IndexedDB and animation frames require a browser environment.
Dynamic loading and image/audio preloading require the corresponding Emscripten
linker options. Individual functions document their requirements and cancellation
behavior. `task::Wget::data(url, method, params)` downloads owned bytes, and
`task::Wget::file(url, file, method, params)` downloads into the virtual filesystem.
Both support GET and POST and abort pending requests when their futures are dropped.
`Wget::legacy_data`, `Wget::legacy_file`, `load_script`, and `preload` are deprecated
because they use legacy operations. `legacy_file` also runs preload plugins.

Use `Wget::data_with_progress(url, method, params, callback)` to receive
`Progress { loaded, total }` updates in bytes (`total` is `None` when unknown).
`Wget::file_with_progress(url, file, method, params, callback)` reports integer
percentages when the total size is known. Callbacks may borrow local state and
run as the download future is polled; dropping the future stops further callbacks.

Task futures and streams use channels local to the calling thread. `channel::mpsc`
provides unbounded and bounded queues; bounded sends return an error when full.
Dropping a timer stream stops its callbacks and releases their state on the next tick.

```rust
use emscripten_futures::task::Idb;

async fn example() -> std::io::Result<()> {
    let db = Idb::new("settings")?;
    db.store("theme", b"dark").await?;
    let theme = db.load("theme").await?;
    Ok(())
}
```

## Async tests

Use `#[emscripten_futures::test]` on an async function to run it with this crate's
`block_on` executor:

```rust
#[emscripten_futures::test]
async fn timer_completes() {
    emscripten_futures::task::sleep(std::time::Duration::from_millis(10)).await;
}
```

The macro generates an ordinary Rust test, preserving test discovery, filtering,
`#[ignore]`, and `#[should_panic]`. Tests can return `Result<(), E>` and use `?`.
They run on the calling thread and can hold non-`Send` values across `.await`.
Test functions must have no arguments or generic parameters. Enable `-sJSPI`
when linking the test executable, as shown above for applications.

## Development

From the source checkout, run `cargo +nightly test` for Node unit tests and
doctests. The browser integration test uses nightly Cargo artifact dependencies
and a locally installed Chrome. On Windows:

```text
cargo +nightly test --target x86_64-pc-windows-msvc --test browser -- --nocapture
```

Use your native target on other platforms. Set `BROWSER` to the browser executable
if automatic discovery does not find it. The browser test fixture is kept in the
source checkout and is excluded from the published package.

Run the proc-macro crate's tests on the native target:

```text
cargo +nightly test -p emscripten-futures-macros --target x86_64-pc-windows-msvc
```

## License

Licensed under either the MIT license or Apache License, Version 2.0, at your option.
See `LICENSE-MIT` and `LICENSE-APACHE`.

The local executor is adapted from the futures-rs project's `futures-executor`
crate, copyright Alex Crichton and The Tokio Authors, under the same licenses.

The local queue is adapted from Actix's `local-channel` 0.1.5, copyright Actix Team,
under the MIT license.
