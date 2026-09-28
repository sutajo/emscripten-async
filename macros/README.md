# emscripten-futures-macros

Async test and benchmark attributes for
[`emscripten-futures`](https://docs.rs/emscripten-futures).
Use the attributes re-exported by that crate; a separate dependency on this
implementation crate is not needed.

## Async tests

```rust,ignore
#[emscripten_futures::test]
async fn timer_completes() {
    emscripten_futures::task::sleep(std::time::Duration::from_millis(10)).await;
}
```

The macro runs a zero-argument async function with `executor::block_on`.
Functions may return `()` or a test-compatible result and may hold non-`Send`
values across `.await`. Ordinary test attributes, including `#[ignore]` and
`#[should_panic]`, are preserved.

## Async benchmarks

```rust,ignore
#![feature(test)]
extern crate test;

#[emscripten_futures::bench]
async fn yield_to_event_loop() {
    emscripten_futures::task::yield_now().await;
}
```

The benchmark macro accepts a zero-argument async function. Each measured
iteration creates a fresh future and runs it to completion with `block_on`.
Measurements include executor overhead and time spent awaiting operations.
Returned values are passed to the harness's black box; returning `Err` does not
automatically fail the benchmark. `#[ignore]` and `#[cfg(...)]` are preserved.
Neither macro accepts generic functions.

Both macros target `wasm32-unknown-emscripten` and require `-sJSPI` when linking
the executable. Benchmarks additionally require nightly Rust and `#![feature(test)]`
in the caller. Async tests can use stable Rust when the runtime's default `spawn`
feature is disabled. See the runtime's documentation for toolchain setup.

Licensed under MIT OR Apache-2.0.
