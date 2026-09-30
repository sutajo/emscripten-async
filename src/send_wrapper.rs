#[allow(unused_imports)]
use libc::pthread_self;

pub struct SendWrapper<T> {
    value: T,
    #[cfg(debug_assertions)]
    owner_thread: libc::pthread_t,
}

impl<T> SendWrapper<T> {
    pub fn new(value: T) -> Self {
        Self {
            value,
            #[cfg(debug_assertions)]
            owner_thread: unsafe { pthread_self() },
        }
    }
}

impl<T> AsRef<T> for SendWrapper<T> {
    #[inline(always)]
    fn as_ref(&self) -> &T {
        #[cfg(debug_assertions)]
        debug_assert_eq!(
            self.owner_thread,
            unsafe { pthread_self() },
            "Value has to be accessed from the owner thread"
        );
        &self.value
    }
}

unsafe impl<T> Send for SendWrapper<T> {}
unsafe impl<T> Sync for SendWrapper<T> {}
