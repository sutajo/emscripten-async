#![cfg(target_os = "emscripten")]

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

pub mod executor;
pub mod promise;
pub mod task;
