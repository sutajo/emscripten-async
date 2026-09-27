# emscripten-futures

A local async executor and awaitable Emscripten operations for Rust applications
targeting `wasm32-unknown-emscripten`.

The executor uses JavaScript Promise Integration (JSPI) to yield to the browser
or Node event loop while Rust futures are pending. It provides `block_on`,
`LocalPool`, and `LocalSpawner` and works with `futures` combinators such as
`join` and `select`. Run the executor and its wakeups on the same thread.

## Usage

Install the Emscripten SDK and the Rust target:

```text
rustup target add wasm32-unknown-emscripten
```

Add the dependency to your application:

```toml
[dependencies]
emscripten-futures = "0.3"
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

- Timers, animation frames, and streams of `Duration` timestamps.
- Downloads to owned bytes or the Emscripten virtual filesystem.
- `Idb` for asynchronous IndexedDB storage.
- Script loading, asset preloading, main-loop blockers, worker replies, and
  dynamic-library loading.

Browser APIs such as IndexedDB and animation frames require a browser environment.
Dynamic loading and image/audio preloading require the corresponding Emscripten
linker options. Individual functions document their requirements and cancellation
behavior. `task::fetch` wraps Emscripten's asynchronous wget-data API.

Task futures and streams use channels local to the calling thread. `task::local_queue`
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
