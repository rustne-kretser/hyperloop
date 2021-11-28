use core::{pin::Pin, sync::atomic::{AtomicBool, Ordering}, task::{Context, Poll}};

use alloc::sync::Arc;
use futures::task::AtomicWaker;

use core::future::Future;


#[derive(Debug)]
struct Shared {
    ready: AtomicBool,
    waker: AtomicWaker,
}

impl Shared {
    fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
            waker: AtomicWaker::new(),
        }
    }
}

pub struct NotificationFuture {
    shared: Arc<Shared>,
}

impl NotificationFuture {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
        }
    }
}

impl Future for NotificationFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.shared.ready.load(Ordering::Relaxed) {
            Poll::Ready(())
        } else {
            self.shared.waker.register(cx.waker());
            Poll::Pending
        }
    }
}

#[derive(Debug)]
pub struct Sender {
    shared: Arc<Shared>,
}

impl Sender {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
        }
    }

    pub fn notify(&self) {
        self.shared.ready.store(true, Ordering::Relaxed);
        self.shared.waker.wake();
    }
}

pub struct Receiver {
    shared: Arc<Shared>,
}

impl Receiver {
    fn new(shared: Arc<Shared>) -> Self {
        Self {
            shared,
        }
    }

    pub fn wait(&self) -> NotificationFuture {
        self.shared.ready.store(false, Ordering::Relaxed);
        NotificationFuture::new(self.shared.clone())
    }
}

pub fn notification() -> (Sender, Receiver) {
    let shared = Arc::new(Shared::new());

    (Sender::new(shared.clone()), Receiver::new(shared))
}

#[cfg(test)]
mod tests {
    use alloc::boxed::Box;
    use crossbeam_queue::ArrayQueue;

    use crate::hyperloop::Hyperloop;

    use super::*;

    #[test]
    fn notify() {
        let (sender, receiver) = notification();

        let mut hyperloop = Hyperloop::<_, 10>::new();
        let queue =  Arc::new(ArrayQueue::new(10));

        async fn wait(receiver: Receiver, queue: Arc<ArrayQueue<u32>>) {
            queue.push(1).unwrap();
            receiver.wait().await;
            queue.push(2).unwrap();
            receiver.wait().await;
            queue.push(3).unwrap();
        }

        hyperloop.add_and_schedule(Box::pin(wait(receiver, queue.clone())), 1);

        hyperloop.poll_tasks();

        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), None);

        hyperloop.poll_tasks();

        assert_eq!(queue.pop(), None);

        sender.notify();

        hyperloop.poll_tasks();

        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);

        hyperloop.poll_tasks();

        assert_eq!(queue.pop(), None);


        sender.notify();

        hyperloop.poll_tasks();

        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), None);
    }
}
