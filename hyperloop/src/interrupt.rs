use core::{
    pin::Pin,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

use core::future::Future;

unsafe fn clone(ptr: *const ()) -> RawWaker {
    RawWaker::new(ptr, &VTABLE)
}

unsafe fn wake(_ptr: *const ()) {}

unsafe fn drop(_ptr: *const ()) {}

const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake, drop);

#[derive(Debug)]
struct YieldFuture {
    done: bool,
}

impl YieldFuture {
    fn new() -> Self {
        Self { done: false }
    }
}

impl Future for YieldFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.done {
            self.done = true;
            cx.waker().wake_by_ref();

            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

pub struct Interrupt<F>
where
    F: Future<Output = ()> + 'static,
{
    future: F,
}

impl<F> Interrupt<F>
where
    F: Future<Output = ()> + 'static,
{
    pub fn new(future_fn: impl FnOnce() -> F) -> Self {
        Self {
            future: future_fn(),
        }
    }

    fn get_waker(&self) -> Waker {
        unsafe { Waker::from_raw(RawWaker::new((self as *const Self).cast(), &VTABLE)) }
    }

    pub fn poll(&'static mut self) {
        let waker = self.get_waker();
        let mut cx = Context::from_waker(&waker);

        let future = unsafe { Pin::new_unchecked(&mut self.future) };

        let _ = future.poll(&mut cx);
    }
}

pub fn yield_now() -> impl Future<Output = ()> {
    YieldFuture::new()
}

#[cfg(test)]
mod tests {
    use core::task::Waker;

    use crossbeam_queue::ArrayQueue;
    use std::{boxed::Box, sync::Arc};

    use crate::common::tests::MockWaker;

    use super::*;

    #[test]
    fn test_yield_now() {
        let mut future = Box::pin(yield_now());
        let mockwaker = Arc::new(MockWaker::new());
        let waker: Waker = mockwaker.into();
        let mut cx = Context::from_waker(&waker);

        assert_eq!(future.as_mut().poll(&mut cx), Poll::Pending);
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(()));
        assert_eq!(future.as_mut().poll(&mut cx), Poll::Ready(()));
    }

    #[test]
    fn test_interrupt() {
        static mut QUEUE: Option<ArrayQueue<u32>> = None;

        fn handler() {
            async fn async_handler() {
                let queue = unsafe { QUEUE.as_ref().unwrap() };

                for i in 0.. {
                    queue.push(i).unwrap();
                    yield_now().await;
                }
            }

            type F = impl Future<Output = ()> + 'static;

            static mut INTERRUPT: Option<Interrupt<F>> = None;

            fn get_future() -> impl FnOnce() -> F {
                || async_handler()
            }

            let interrupt = unsafe { INTERRUPT.get_or_insert(Interrupt::new(get_future())) };

            interrupt.poll();
        }

        unsafe {
            QUEUE = Some(ArrayQueue::new(10));
        };

        let queue = unsafe { QUEUE.as_ref().unwrap() };

        handler();

        assert_eq!(queue.pop(), Some(0));
        assert_eq!(queue.pop(), None);

        handler();
        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), None);

        handler();
        handler();
        handler();
        handler();
        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), Some(4));
        assert_eq!(queue.pop(), Some(5));
        assert_eq!(queue.pop(), None);
    }
}
