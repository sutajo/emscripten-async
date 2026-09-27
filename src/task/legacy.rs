use super::{Completion, receive};
use emscripten_functions_sys::emscripten as ffi;
use futures::channel::oneshot;
use std::{
    ffi::{CString, c_char},
    io,
};

// These legacy APIs have no user-data argument. Serialize them so their shared
// callbacks can identify the request. The callback owns the lock, so canceling
// an await cannot accidentally deliver its response to the next request.
static LEGACY_LOCK: futures::lock::Mutex<()> = futures::lock::Mutex::new(());
static LEGACY_REQUEST: std::sync::Mutex<Option<LegacyRequest>> = std::sync::Mutex::new(None);

pub(super) enum LegacyOperation {
    Download(CString, CString),
    LoadScript(CString),
    Preload(CString),
}

struct LegacyRequest {
    _operation: LegacyOperation,
    sender: Completion<()>,
    _guard: futures::lock::MutexGuard<'static, ()>,
}

fn legacy_complete(result: io::Result<()>) {
    let request = LEGACY_REQUEST.lock().unwrap().take().unwrap();
    let _ = request.sender.send(result);
}

unsafe extern "C" fn legacy_success() {
    legacy_complete(Ok(()));
}
unsafe extern "C" fn legacy_error() {
    legacy_complete(Err(io::Error::other("Emscripten operation failed")));
}
unsafe extern "C" fn legacy_file_success(_: *const c_char) {
    legacy_complete(Ok(()));
}
unsafe extern "C" fn legacy_file_error(_: *const c_char) {
    legacy_complete(Err(io::Error::other("file operation failed")));
}

pub(super) async fn legacy(operation: LegacyOperation) -> io::Result<()> {
    let guard = LEGACY_LOCK.lock().await;
    let (sender, receiver) = oneshot::channel();
    let (kind, first, second) = match &operation {
        LegacyOperation::Download(url, file) => (0, url.as_ptr(), file.as_ptr()),
        LegacyOperation::LoadScript(url) => (1, url.as_ptr(), std::ptr::null()),
        LegacyOperation::Preload(file) => (2, file.as_ptr(), std::ptr::null()),
    };
    *LEGACY_REQUEST.lock().unwrap() = Some(LegacyRequest {
        _operation: operation,
        sender,
        _guard: guard,
    });
    // The installed request owns every pointer, even if the future is dropped.
    // No mutex is held across a call that may invoke a callback synchronously.
    unsafe {
        match kind {
            0 => ffi::emscripten_async_wget(
                first,
                second,
                Some(legacy_file_success),
                Some(legacy_file_error),
            ),
            1 => ffi::emscripten_async_load_script(first, Some(legacy_success), Some(legacy_error)),
            _ => {
                if ffi::emscripten_run_preload_plugins(
                    first,
                    Some(legacy_file_success),
                    Some(legacy_file_error),
                ) != 0
                {
                    legacy_complete(Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        "preload file not found",
                    )));
                }
            }
        }
    }
    receive(receiver).await
}
