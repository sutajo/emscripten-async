//! Awaitable Emscripten operations. Unless stated otherwise, dropping a future
//! leaves its operation running and its completion callback releases the state.
//! Browser-only APIs retain their browser requirements (XHR, IndexedDB, DOM, etc.).
//! Task channels are local to the calling thread.
//!
//! | Operation | Task API |
//! | --- | --- |
//! | async call / timeout / immediate | [`sleep`], [`timeout`], [`yield_now`] |
//! | repeating timers and frames | [`interval`], [`timeout_loop`], [`immediate_loop`], [`animation_frames`] |
//! | next animation frame | [`animation_frame`] |
//! | counted / uncounted main-loop blockers | [`main_loop_blocker`] |
//! | wget / wget_data / wget2 / wget2_data | [`wget`], [`fetch`], [`wget2`], [`wget2_data`] |
//! | load JavaScript | [`load_script`] |
//! | IndexedDB async operations | [`Idb`] |
//! | file / data preload plugins | [`preload`], [`preload_data`] |
//! | worker request/reply | [`call_worker`] (one final reply; see safety contract) |
//! | asynchronous dynamic loading | [`dlopen`] |
//!
//! These wrap task-style operations in the installed bindings. DOM/socket event
//! registrations, Promise combinators, and the variadic EM_ASM compiler hooks
//! remain available through the underlying bindings rather than this task API.

mod download;
mod dylib;
mod indexed_db;
mod legacy;
mod preload;
mod script;
mod timer;
mod worker;

pub use download::{fetch, wget, wget2, wget2_data};
pub use dylib::{Library, dlopen};
pub use indexed_db::Idb;
pub use preload::{preload, preload_data};
pub use script::load_script;
pub use timer::{
    Ticks, animation_frame, animation_frames, immediate_loop, interval, main_loop_blocker, sleep,
    timeout, timeout_loop, yield_now,
};
pub use worker::call_worker;

#[cfg(test)]
mod tests;

use crate::channel::mpsc;
use std::{
    ffi::{CString, c_void},
    io,
};

type Completion<T> = mpsc::Sender<io::Result<T>>;

fn c_string(value: &str) -> io::Result<CString> {
    CString::new(value).map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))
}

fn completion<T>() -> (*mut c_void, mpsc::Receiver<io::Result<T>>) {
    let (sender, receiver) = mpsc::bounded::<io::Result<T>>(1);
    (Box::into_raw(Box::new(sender)).cast(), receiver)
}

// arg must be a sender from completion::<T>(), consumed by exactly one callback.
unsafe fn complete<T>(arg: *mut c_void, result: io::Result<T>) {
    let sender = unsafe { Box::from_raw(arg.cast::<Completion<T>>()) };
    let _ = sender.send(result);
}

unsafe extern "C" fn failed<T>(arg: *mut c_void) {
    unsafe { complete::<T>(arg, Err(io::Error::other("Emscripten operation failed"))) };
}

unsafe extern "C" fn succeeded(arg: *mut c_void) {
    unsafe { complete(arg, Ok(())) };
}

async fn receive<T>(mut receiver: mpsc::Receiver<io::Result<T>>) -> io::Result<T> {
    receiver
        .recv()
        .await
        .ok_or_else(|| io::Error::other("operation canceled"))?
}

// Nonempty buffers must contain len readable bytes for the duration of the call.
unsafe fn copy_bytes(data: *const c_void, len: usize) -> io::Result<Vec<u8>> {
    if len == 0 {
        return Ok(Vec::new());
    }
    if data.is_null() || len > isize::MAX as usize {
        return Err(io::Error::other("invalid or oversized response buffer"));
    }
    Ok(unsafe { std::slice::from_raw_parts(data.cast::<u8>(), len) }.to_vec())
}

unsafe extern "C" fn loaded_bytes(arg: *mut c_void, data: *mut c_void, len: i32) {
    let result = match usize::try_from(len) {
        Ok(len) => unsafe { copy_bytes(data, len) },
        Err(_) => Err(io::Error::other("response exceeds i32::MAX bytes")),
    };
    unsafe { complete(arg, result) };
}
