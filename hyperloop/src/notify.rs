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
    use crossbeam_queue::ArrayQueue;

    use crate::{executor::Executor, task::Task};

    use super::*;

    #[test]
    fn notify() {
        let (sender, receiver) = notification();

        let mut executor = Executor::<10>::new();
        let queue =  Arc::new(ArrayQueue::new(10));

        let wait = |receiver, queue| {
            move || {
                async fn future(receiver: Receiver, queue: Arc<ArrayQueue<u32>>) {
                    queue.push(1).unwrap();
                    receiver.wait().await;
                    queue.push(2).unwrap();
                    receiver.wait().await;
                    queue.push(3).unwrap();
                }
                future(receiver, queue)
            }
        };

        let task1 = Task::new(wait(receiver, queue.clone()), 1);

        task1.add_to_executor(executor.get_sender()).unwrap();

        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), None);

        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), None);

        sender.notify();

        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);

        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), None);


        sender.notify();

        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), None);
    }
}
