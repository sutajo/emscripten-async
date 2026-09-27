//! Bounded local queue adapted from local-channel 0.1.5's src/mpsc.rs:
//! https://docs.rs/local-channel/0.1.5/src/local_channel/mpsc.rs.html
//!
//! Copyright (c) 2017-NOW Actix Team. Used under the MIT license; see
//! ../../LICENSE-MIT (which includes the upstream copyright notice).
//!
//! Modified to add bounded channels and release state borrows before invoking
//! wakers or dropping queued values.

use core::{
    cell::RefCell,
    fmt,
    future::poll_fn,
    pin::Pin,
    task::{Context, Poll},
};
use std::error::Error;
use std::{collections::VecDeque, rc::Rc};

use futures::{Sink, Stream};
use local_waker::LocalWaker;

/// Creates a unbounded in-memory channel with buffered storage.
///
/// [Sender]s and [Receiver]s are `!Send`.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    bounded(usize::MAX)
}

/// Creates a channel with at most `capacity` queued messages across all senders.
/// Sending to a full buffer returns an error, including through `Sink`.
/// Panics if `capacity` is zero.
pub fn bounded<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
    assert!(capacity > 0);
    let shared = Rc::new(RefCell::new(Shared {
        has_receiver: true,
        buffer: VecDeque::new(),
        blocked_recv: LocalWaker::new(),
        capacity,
    }));

    let sender = Sender {
        shared: shared.clone(),
    };

    let receiver = Receiver { shared };

    (sender, receiver)
}

#[derive(Debug)]
struct Shared<T> {
    buffer: VecDeque<T>,
    blocked_recv: LocalWaker,
    has_receiver: bool,
    capacity: usize,
}

/// The transmission end of a channel.
///
/// This is created by the `channel` function.
#[derive(Debug)]
pub struct Sender<T> {
    shared: Rc<RefCell<Shared<T>>>,
}

impl<T> Unpin for Sender<T> {}

impl<T> Sender<T> {
    /// Sends the provided message along this channel.
    pub fn send(&self, item: T) -> Result<(), SendError<T>> {
        let mut shared = self.shared.borrow_mut();

        if !shared.has_receiver {
            // receiver was dropped
            return Err(SendError(item, false));
        };

        if shared.buffer.len() == shared.capacity {
            return Err(SendError(item, true));
        }

        shared.buffer.push_back(item);
        let waker = shared.blocked_recv.take();
        drop(shared);
        if let Some(waker) = waker {
            waker.wake();
        }

        Ok(())
    }

    /// Closes the sender half.
    ///
    /// This prevents any further messages from being sent on the channel, by any sender, while
    /// still enabling the receiver to drain messages that are already buffered.
    pub fn close(&mut self) {
        let mut shared = self.shared.borrow_mut();
        shared.has_receiver = false;
        let waker = shared.blocked_recv.take();
        drop(shared);
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        Sender {
            shared: self.shared.clone(),
        }
    }
}

impl<T> Sink<T> for Sender<T> {
    type Error = SendError<T>;

    fn poll_ready(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn start_send(self: Pin<&mut Self>, item: T) -> Result<(), SendError<T>> {
        self.send(item)
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), SendError<T>>> {
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let count = Rc::strong_count(&self.shared);
        let mut shared = self.shared.borrow_mut();

        // check is last sender is about to drop
        if shared.has_receiver && count == 2 {
            // The dropping sender still owns its Rc during an inline wake.
            shared.has_receiver = false;
            // Wake up receiver as its stream has ended
            let waker = shared.blocked_recv.take();
            drop(shared);
            if let Some(waker) = waker {
                waker.wake();
            }
        }
    }
}

/// The receiving end of a channel which implements the `Stream` trait.
///
/// This is created by the [`channel`] function.
#[derive(Debug)]
pub struct Receiver<T> {
    shared: Rc<RefCell<Shared<T>>>,
}

impl<T> Receiver<T> {
    /// Receive the next value.
    ///
    /// Returns `None` if the channel is empty and has been [closed](Sender::close) explicitly or
    /// when all senders have been dropped and, therefore, no more values can ever be sent though
    /// this channel.
    pub async fn recv(&mut self) -> Option<T> {
        let mut this = Pin::new(self);
        poll_fn(|cx| this.as_mut().poll_next(cx)).await
    }

    /// Create an associated [Sender].
    pub fn sender(&self) -> Sender<T> {
        Sender {
            shared: self.shared.clone(),
        }
    }
}

impl<T> Unpin for Receiver<T> {}

impl<T> Stream for Receiver<T> {
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut shared = self.shared.borrow_mut();

        if !shared.has_receiver || Rc::strong_count(&self.shared) == 1 {
            // All senders have been dropped, so drain the buffer and end the stream.
            return Poll::Ready(shared.buffer.pop_front());
        }

        if let Some(msg) = shared.buffer.pop_front() {
            Poll::Ready(Some(msg))
        } else {
            // Registering can clone/drop wakers, which may reenter the queue.
            drop(shared);
            let blocked_recv = LocalWaker::new();
            blocked_recv.register(cx.waker());
            let previous = {
                let mut shared = self.shared.borrow_mut();
                std::mem::replace(&mut shared.blocked_recv, blocked_recv)
            };
            drop(previous);

            // Clone/drop callbacks may have delivered a value or closed the
            // channel before registration finished. Do not miss that change.
            let mut shared = self.shared.borrow_mut();
            if !shared.has_receiver || Rc::strong_count(&self.shared) == 1 {
                Poll::Ready(shared.buffer.pop_front())
            } else if let Some(msg) = shared.buffer.pop_front() {
                Poll::Ready(Some(msg))
            } else {
                Poll::Pending
            }
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let mut shared = self.shared.borrow_mut();
        shared.has_receiver = false;
        let buffer = std::mem::take(&mut shared.buffer);
        let waker = shared.blocked_recv.take();
        drop(shared);
        drop(buffer);
        drop(waker);
    }
}

/// Error returned when the buffer is full or the receiver is dropped or closed.
///
/// Allows access to message that failed to send with [`into_inner`](Self::into_inner).
pub struct SendError<T>(pub T, bool);

impl<T> SendError<T> {
    /// Whether the buffer is full rather than closed.
    pub fn is_full(&self) -> bool {
        self.1
    }

    /// Returns the message that was attempted to be sent but failed.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for SendError<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_tuple("SendError").field(&"...").finish()
    }
}

impl<T> fmt::Display for SendError<T> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_full() {
            write!(fmt, "send failed because buffer is full")
        } else {
            write!(fmt, "send failed because receiver is gone")
        }
    }
}

impl<T> Error for SendError<T> {}

#[cfg(test)]
mod tests {
    use futures::{StreamExt as _, future::lazy};

    use super::*;

    #[crate::test]
    async fn test_mpsc() {
        let (tx, mut rx) = channel();
        tx.send("test").unwrap();
        assert_eq!(rx.next().await.unwrap(), "test");

        let tx2 = tx.clone();
        tx2.send("test2").unwrap();
        assert_eq!(rx.next().await.unwrap(), "test2");

        assert_eq!(
            lazy(|cx| Pin::new(&mut rx).poll_next(cx)).await,
            Poll::Pending
        );
        drop(tx2);
        assert_eq!(
            lazy(|cx| Pin::new(&mut rx).poll_next(cx)).await,
            Poll::Pending
        );
        drop(tx);
        assert_eq!(rx.next().await, None);

        let (tx, rx) = channel();
        tx.send("test").unwrap();
        drop(rx);
        assert!(tx.send("test").is_err());

        let (mut tx, _) = channel();
        let tx2 = tx.clone();
        tx.close();
        assert!(tx.send("test").is_err());
        assert!(tx2.send("test").is_err());
    }

    #[crate::test]
    async fn test_recv() {
        let (tx, mut rx) = channel();
        tx.send("test").unwrap();
        assert_eq!(rx.recv().await.unwrap(), "test");
        drop(tx);

        let (tx, mut rx) = channel();
        tx.send("test").unwrap();
        assert_eq!(rx.recv().await.unwrap(), "test");
        drop(tx);
        assert!(rx.recv().await.is_none());
    }

    #[crate::test]
    async fn test_bounded() {
        let (tx, mut rx) = bounded(1);
        let mut tx2 = tx.clone();
        tx.send(1).unwrap();
        let error = tx2.send(2).unwrap_err();
        assert!(error.is_full());
        assert_eq!(error.into_inner(), 2);
        assert_eq!(rx.recv().await, Some(1));
        tx2.send(2).unwrap();
        tx2.close();
        assert!(!tx.send(3).unwrap_err().is_full());
        assert_eq!(rx.recv().await, Some(2));
        assert_eq!(rx.recv().await, None);

        let (tx, rx) = bounded(1);
        tx.send(1).unwrap();
        drop(rx);
        assert!(!tx.send(2).unwrap_err().is_full());
    }

    thread_local! {
        static ON_CLONE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static ON_DROP: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
        static ON_WAKE: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
    }

    fn callback_waker() -> std::task::Waker {
        use std::task::{RawWaker, RawWakerVTable, Waker};
        unsafe fn clone(_: *const ()) -> RawWaker {
            let callback = ON_CLONE.with(|slot| slot.borrow_mut().take());
            if let Some(callback) = callback {
                callback();
            }
            RawWaker::new(std::ptr::null(), &VTABLE)
        }
        unsafe fn drop(_: *const ()) {
            let callback = ON_DROP.with(|slot| slot.borrow_mut().take());
            if let Some(callback) = callback {
                callback();
            }
        }
        unsafe fn wake(_: *const ()) {
            let callback = ON_WAKE.with(|slot| slot.borrow_mut().take());
            if let Some(callback) = callback {
                callback();
            }
        }
        static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake, drop);
        // No owned raw data; callbacks use only the calling thread's storage.
        unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VTABLE)) }
    }

    #[test]
    fn waker_clone_and_drop_can_reenter() {
        for on_clone in [true, false] {
            let (tx, mut rx) = bounded(1);
            let waker = callback_waker();
            let mut cx = Context::from_waker(&waker);
            assert!(Pin::new(&mut rx).poll_next(&mut cx).is_pending());
            let callback: Box<dyn FnOnce()> = Box::new(move || tx.send(7).unwrap());
            if on_clone {
                ON_CLONE.with(|slot| *slot.borrow_mut() = Some(callback));
            } else {
                ON_DROP.with(|slot| *slot.borrow_mut() = Some(callback));
            }
            assert_eq!(Pin::new(&mut rx).poll_next(&mut cx), Poll::Ready(Some(7)));
            assert_eq!(Pin::new(&mut rx).poll_next(&mut cx), Poll::Ready(None));
        }
    }

    #[test]
    fn last_sender_drop_can_wake_and_poll_inline() {
        let (tx, mut rx) = channel::<()>();
        let waker = callback_waker();
        assert!(
            Pin::new(&mut rx)
                .poll_next(&mut Context::from_waker(&waker))
                .is_pending()
        );
        ON_WAKE.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move || {
                let mut cx = Context::from_waker(std::task::Waker::noop());
                assert_eq!(Pin::new(&mut rx).poll_next(&mut cx), Poll::Ready(None));
            }))
        });
        drop(tx);
        ON_WAKE.with(|slot| assert!(slot.borrow().is_none()));
    }

    #[test]
    fn queued_values_drop_outside_the_borrow() {
        struct Value(Sender<Value>);
        impl Drop for Value {
            fn drop(&mut self) {
                drop(self.0.clone());
            }
        }
        let (tx, rx) = bounded(1);
        tx.send(Value(tx.clone())).unwrap();
        drop(rx);
        drop(tx);
    }
}
