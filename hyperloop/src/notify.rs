use core::{
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};

use futures::task::AtomicWaker;

use core::future::Future;

#[derive(Debug)]
pub struct Notification {
    ready: AtomicBool,
    waker: AtomicWaker,
}

impl Notification {
    pub const fn new() -> Self {
        Self {
            ready: AtomicBool::new(false),
            waker: AtomicWaker::new(),
        }
    }

    pub fn notify(&self) {
        self.ready.store(true, Ordering::Relaxed);
        self.waker.wake();
    }

    pub fn wait(&'static self) -> NotificationFuture {
        self.ready.store(false, Ordering::Relaxed);
        NotificationFuture::new(&self)
    }
}

pub struct NotificationFuture {
    notification: &'static Notification,
}

impl NotificationFuture {
    fn new(shared: &'static Notification) -> Self {
        Self {
            notification: shared,
        }
    }
}

impl Future for NotificationFuture {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.notification.ready.load(Ordering::Relaxed) {
            Poll::Ready(())
        } else {
            self.notification.waker.register(cx.waker());
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use crossbeam_queue::ArrayQueue;
    use std::boxed::Box;
    use std::sync::Arc;

    use crate::{
        executor::{Executor, ExecutorHandle},
        task::Task,
    };

    use super::*;

    #[test]
    fn notify() {
        let notification = Box::leak(Box::new(Notification::new()));

        let queue = Arc::new(ArrayQueue::new(10));

        let wait = |receiver, queue| {
            move || {
                async fn future(notification: &'static Notification, queue: Arc<ArrayQueue<u32>>) {
                    queue.push(1).unwrap();
                    notification.wait().await;
                    queue.push(2).unwrap();
                    notification.wait().await;
                    queue.push(3).unwrap();
                }
                future(receiver, queue)
            }
        };

        let task = Box::leak(Box::new(Task::new(wait(notification, queue.clone()), 1)));

        let mut executor =
            ExecutorHandle::new(Box::leak(Box::new(Executor::new([task.get_handle()]))));

        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), None);

        executor.poll_tasks();

        assert_eq!(queue.pop(), None);

        notification.notify();

        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);

        executor.poll_tasks();

        assert_eq!(queue.pop(), None);

        notification.notify();

        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), None);
    }
}
