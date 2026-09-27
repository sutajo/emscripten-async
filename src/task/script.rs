use super::c_string;
use super::legacy::{LegacyOperation, legacy};
use std::io;

/// Loads and evaluates a script, waiting for its Emscripten run dependencies.
/// Requires the main browser thread (or Emscripten's Node script loader).
/// Legacy file/script operations are serialized, including after cancellation.
///
/// # Safety
/// Loaded code must preserve Rust memory safety. It must not throw across Rust frames.
#[deprecated(note = "uses serialized legacy script-loading callbacks")]
pub async unsafe fn load_script(url: &str) -> io::Result<()> {
    legacy(LegacyOperation::LoadScript(c_string(url)?)).await
}
