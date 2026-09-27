use super::{c_string, complete, completion, failed, loaded_bytes, receive, succeeded};
use emscripten_rs_sys as ffi;
use std::{
    ffi::{CString, c_void},
    io,
};

/// A named IndexedDB store for owned byte values.
///
/// Requires IndexedDB in the calling environment. The name is validated once;
/// Emscripten opens the database as needed when an operation starts.
#[derive(Clone, Debug)]
pub struct Idb {
    name: CString,
}

impl Idb {
    /// Selects a database without opening it. Rejects names containing NUL bytes.
    pub fn new(name: &str) -> io::Result<Self> {
        Ok(Self {
            name: c_string(name)?,
        })
    }

    /// Loads bytes from IndexedDB. Requires IndexedDB in the calling environment.
    pub async fn load(&self, key: &str) -> io::Result<Vec<u8>> {
        let key = c_string(key)?;
        let (arg, receiver) = completion::<Vec<u8>>();
        // The API copies names synchronously; its callback buffer is borrowed.
        unsafe {
            ffi::emscripten_idb_async_load(
                self.name.as_ptr(),
                key.as_ptr(),
                arg,
                Some(loaded_bytes),
                Some(failed::<Vec<u8>>),
            );
        }
        receive(receiver).await
    }

    /// Stores bytes in IndexedDB. Emscripten copies the input before returning.
    pub async fn store(&self, key: &str, data: &[u8]) -> io::Result<()> {
        let key = c_string(key)?;
        let len = i32::try_from(data.len())
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
        let (arg, receiver) = completion::<()>();
        unsafe {
            ffi::emscripten_idb_async_store(
                self.name.as_ptr(),
                key.as_ptr(),
                data.as_ptr().cast_mut().cast(),
                len,
                arg,
                Some(succeeded),
                Some(failed::<()>),
            );
        }
        receive(receiver).await
    }

    /// Deletes an IndexedDB entry.
    pub async fn delete(&self, key: &str) -> io::Result<()> {
        let key = c_string(key)?;
        let (arg, receiver) = completion::<()>();
        unsafe {
            ffi::emscripten_idb_async_delete(
                self.name.as_ptr(),
                key.as_ptr(),
                arg,
                Some(succeeded),
                Some(failed::<()>),
            );
        }
        receive(receiver).await
    }

    /// Checks whether an IndexedDB entry exists.
    pub async fn exists(&self, key: &str) -> io::Result<bool> {
        unsafe extern "C" fn exists(arg: *mut c_void, value: i32) {
            unsafe { complete(arg, Ok(value != 0)) };
        }
        let key = c_string(key)?;
        let (arg, receiver) = completion::<bool>();
        unsafe {
            ffi::emscripten_idb_async_exists(
                self.name.as_ptr(),
                key.as_ptr(),
                arg,
                Some(exists),
                Some(failed::<bool>),
            );
        }
        receive(receiver).await
    }

    /// Removes all entries from the database's Emscripten object store.
    pub async fn clear(&self) -> io::Result<()> {
        let (arg, receiver) = completion::<()>();
        unsafe {
            ffi::emscripten_idb_async_clear(
                self.name.as_ptr(),
                arg,
                Some(succeeded),
                Some(failed::<()>),
            )
        };
        receive(receiver).await
    }
}
