Run `cargo +nightly test --lib` for the Node unit tests.

The native integration test builds a separate Emscripten test binary and locates
it through `CARGO_BIN_FILE_BROWSER_TEST_BIN`. It launches headless Chrome with a
temporary profile, serves the binary and fixtures on localhost, and checks the
exit status and completion message reported by the browser.

Run it with your native Rust target, for example on Windows:

```text
cargo +nightly test --target x86_64-pc-windows-msvc --test browser -- --nocapture
```

Use `rustc -vV` to find your host target. Cargo's nightly artifact dependencies
are enabled in `.cargo/config.toml`. Chrome must support WebAssembly JSPI.
Set `BROWSER` to its executable if automatic discovery does not find it.

The browser tests cover binary, empty, missing, POST, and canceled downloads,
IndexedDB, legacy callback routing after cancellation, scripts, preloading,
animation frames, and worker replies. Failures and a 45-second timeout fail the
integration test; the browser is stopped when the test exits.

Dynamic loading is compile-checked but requires a separate application linked
with `-sMAIN_MODULE` and a compatible side module for runtime testing. Image/audio
decoding requires the corresponding Emscripten preload plugins; these tests
exercise the generic binary preload path.
