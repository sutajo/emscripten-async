#![cfg(target_os = "emscripten")]
#![doc = include_str!("../README.md")]

extern crate self as emscripten_futures;

/// Runs an async test using [`executor::block_on`].
///
/// Apply this attribute to an async function with no arguments or generic
/// parameters. The function may return `()` or a test-compatible type such as
/// `Result<(), E>`. Futures need not be `Send`; they run on the calling thread.
/// Ordinary test attributes, including `#[ignore]` and `#[should_panic]`, work
/// as usual. The test executable must be linked with Emscripten's `-sJSPI` flag.
///
/// ```no_run
/// #[emscripten_futures::test]
/// async fn timer_completes() {
///     emscripten_futures::task::sleep(std::time::Duration::from_millis(10)).await;
/// }
/// ```
pub use emscripten_futures_macros::test;

/// Benchmarks an async function using [`executor::block_on`].
///
/// Apply to an async function with no arguments. The macro calls `Bencher::iter`
/// automatically, awaiting each measured iteration through `block_on`. Each
/// iteration creates a fresh future and runs it to completion on the calling
/// thread; futures and return values need not be `Send`. The measured time includes
/// executor overhead and time spent awaiting operations. Return values are passed
/// to the harness's black box; an `Err` is not automatically treated as a failure.
/// Attributes such as `#[ignore]` and `#[cfg(...)]` are preserved.
/// Functions must not have generic parameters.
///
/// Requires nightly Rust, `#![feature(test)]`, and `extern crate test` in the
/// benchmark crate, plus `-sJSPI` at link time. The `spawn` feature is not required.
/// Run with `cargo +nightly bench`.
///
/// ```rust,ignore
/// #![feature(test)]
/// extern crate test;
///
/// #[emscripten_futures::bench]
/// async fn yield_to_event_loop() {
///     emscripten_futures::task::yield_now().await;
/// }
/// ```
pub use emscripten_futures_macros::bench;

pub mod channel;
pub mod executor;
pub mod task;
mod send_wrapper;