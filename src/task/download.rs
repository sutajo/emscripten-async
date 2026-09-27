use super::legacy::{LegacyOperation, legacy};
use super::{
    Completion, c_string, completion, copy_bytes, failed, loaded_bytes, local_queue, receive,
};
use emscripten_functions_sys::emscripten as ffi;
use std::{
    cell::RefCell,
    ffi::{CStr, CString, c_char, c_void},
    io,
};

/// Downloads a URL into owned bytes using an asynchronous GET request.
///
/// Dropping the future leaves the request running; its callback cleans up.
/// Returns an error if the URL contains a NUL byte or the download fails.
pub async fn fetch(url: &str) -> io::Result<Vec<u8>> {
    let url = c_string(url)?;
    let (arg, receiver) = completion::<Vec<u8>>();

    // SAFETY: Emscripten copies the URL during this call. The callbacks own the
    // sender independently of the future, including when the future is dropped.
    unsafe {
        ffi::emscripten_async_wget_data(
            url.as_ptr(),
            arg,
            Some(loaded_bytes),
            Some(failed::<Vec<u8>>),
        )
    };
    receive(receiver).await
}

/// Downloads a URL into the virtual filesystem and runs preload plugins.
/// Uses `emscripten_async_wget`. Legacy file/script operations are serialized.
pub async fn wget(url: &str, file: &str) -> io::Result<()> {
    legacy(LegacyOperation::Download(c_string(url)?, c_string(file)?)).await
}

// wget2's abort callback does not call onerror. Keep ownership in a guard so
// cancellation can abort the request and free its sender without a leak.
struct DownloadGuard<T> {
    handle: i32,
    sender: Box<RefCell<Option<Completion<T>>>>,
}

impl<T> Drop for DownloadGuard<T> {
    fn drop(&mut self) {
        // Always synchronize with the native request before freeing user data.
        unsafe { ffi::emscripten_async_wget2_abort(self.handle) };
    }
}

unsafe fn download_complete<T>(arg: *mut c_void, result: io::Result<T>) {
    let sender = unsafe { &*arg.cast::<RefCell<Option<Completion<T>>>>() }
        .borrow_mut()
        .take();
    // Release the callback state borrow before notifying the receiver.
    if let Some(sender) = sender {
        let _ = sender.send(result);
    }
}

unsafe extern "C" fn download_bytes(_: u32, arg: *mut c_void, data: *mut c_void, len: u32) {
    let result = unsafe { copy_bytes(data, len as usize) };
    unsafe { download_complete(arg, result) };
}

unsafe extern "C" fn download_file(_: u32, arg: *mut c_void, _: *const c_char) {
    unsafe { download_complete(arg, Ok(())) };
}

unsafe extern "C" fn download_error<T>(_: u32, arg: *mut c_void, status: i32) {
    unsafe {
        download_complete::<T>(
            arg,
            Err(io::Error::other(format!("HTTP request failed: {status}"))),
        )
    };
}

unsafe extern "C" fn download_data_error(
    _: u32,
    arg: *mut c_void,
    status: i32,
    message: *const c_char,
) {
    let message = if message.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };
    unsafe {
        download_complete::<Vec<u8>>(
            arg,
            Err(io::Error::other(format!(
                "HTTP request failed: {status} {message}"
            ))),
        )
    };
}

fn download_state<T>() -> (DownloadGuard<T>, local_queue::Receiver<io::Result<T>>) {
    let (sender, receiver) = local_queue::bounded(1);
    (
        DownloadGuard {
            handle: -1,
            sender: Box::new(RefCell::new(Some(sender))),
        },
        receiver,
    )
}

/// Downloads bytes using `emscripten_async_wget2_data` and XMLHttpRequest.
/// `method` is GET or POST; POST `params` are form-encoded text.
/// Dropping a pending future aborts the request. Requires browser XHR.
pub async fn wget2_data(url: &str, method: &str, params: &str) -> io::Result<Vec<u8>> {
    let url = c_string(url)?;
    let method = request_method(method)?;
    let params = c_string(params)?;
    let (mut request, receiver) = download_state::<Vec<u8>>();
    let arg = (&*request.sender as *const RefCell<Option<Completion<Vec<u8>>>>)
        .cast_mut()
        .cast();
    request.handle = unsafe {
        ffi::emscripten_async_wget2_data(
            url.as_ptr(),
            method.as_ptr(),
            params.as_ptr(),
            arg,
            1,
            Some(download_bytes),
            Some(download_data_error),
            None,
        )
    };
    let result = receive(receiver).await;
    drop(request);
    result
}

/// Downloads into the virtual filesystem using `emscripten_async_wget2`.
/// Unlike `wget`, this does not run preload plugins. Supports GET and POST.
/// Dropping a pending future aborts the request. Requires browser XHR.
pub async fn wget2(url: &str, file: &str, method: &str, params: &str) -> io::Result<()> {
    let url = c_string(url)?;
    let file = c_string(file)?;
    let method = request_method(method)?;
    let params = c_string(params)?;
    let (mut request, receiver) = download_state::<()>();
    let arg = (&*request.sender as *const RefCell<Option<Completion<()>>>)
        .cast_mut()
        .cast();
    request.handle = unsafe {
        ffi::emscripten_async_wget2(
            url.as_ptr(),
            file.as_ptr(),
            method.as_ptr(),
            params.as_ptr(),
            arg,
            Some(download_file),
            Some(download_error::<()>),
            None,
        )
    };
    let result = receive(receiver).await;
    drop(request);
    result
}

fn request_method(method: &str) -> io::Result<CString> {
    match method {
        "GET" | "POST" => c_string(method),
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected GET or POST",
        )),
    }
}
