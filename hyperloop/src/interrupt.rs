use core::{pin::Pin, task::{Context, Poll, Waker}};

use core::future::Future;

use alloc::{boxed::Box, sync::Arc, task::Wake};

struct InterruptWaker {}

impl InterruptWaker {
    fn new() -> Self {
        Self { }
    }
}

impl Wake for InterruptWaker {
    fn wake(self: alloc::sync::Arc<Self>) {
    }
}

#[derive(Debug)]
struct YieldFuture {
    done: bool,
}

impl YieldFuture {
    fn new() -> Self {
        Self {
            done: false
        }
    }
}

impl Future for YieldFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.done {
            self.done = true;
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }
}

pub struct Interrupt {
    future: Pin<Box<dyn Future<Output = ()> + Sync>>,
    waker: Waker,
}

impl Interrupt {
    pub fn new(future: Pin<Box<dyn Future<Output = ()> + Sync>>) -> Self {
        Self {
            future,
            waker: Arc::new(InterruptWaker::new()).into(),
        }
    }

    pub fn poll(&mut self) {
        let mut cx = Context::from_waker(&self.waker);

        let _ = self.future.as_mut().poll(&mut cx);
    }
}

pub fn yield_now() -> impl Future<Output = ()> {
    YieldFuture::new()
}

#[cfg(test)]
mod tests {
    use core::task::Waker;

    use alloc::{boxed::Box, sync::Arc};
    use crossbeam_queue::ArrayQueue;

    use super::*;

    #[test]
    fn test_yield_now() {
        let mut future = Box::pin(yield_now());
        let mockwaker = Arc::new(InterruptWaker::new());
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

            static mut INTERRUPT: Option<Interrupt> = None;

            let interrupt = unsafe {
                INTERRUPT.get_or_insert(Interrupt::new(Box::pin(async_handler())))
            };

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
