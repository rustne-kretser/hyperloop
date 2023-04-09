use core::{
    pin::Pin,
    task::{Context, Poll, Waker},
};

use futures::Future;

use crate::executor::Priority;

pub struct Task<F>
where
    F: Future<Output = ()> + 'static,
{
    future: F,
    priority: Priority,
}

impl<F> Task<F>
where
    F: Future<Output = ()> + 'static,
{
    pub fn new(future_fn: impl FnOnce() -> F, priority: Priority) -> Self {
        Self {
            future: future_fn(),
            priority,
        }
    }

    pub fn get_handle(&'static mut self) -> TaskHandle {
        TaskHandle::new(self)
    }
}

pub struct TaskHandle {
    future: *mut (),
    poll: fn(*mut (), &mut Context<'_>) -> Poll<()>,
    pub priority: Priority,
}

impl TaskHandle {
    pub fn new<F: Future<Output = ()>>(task: &'static mut Task<F>) -> Self {
        unsafe {
            Self {
                future: core::mem::transmute::<_, _>(&mut task.future),
                poll: core::mem::transmute::<
                    fn(Pin<&mut F>, &mut Context<'_>) -> Poll<F::Output>,
                    fn(*mut (), &mut Context<'_>) -> Poll<()>,
                >(F::poll),
                priority: task.priority,
            }
        }
    }

    pub fn poll(&mut self, waker: Waker) -> Poll<()> {
        let mut cx = Context::from_waker(&waker);

        let poll = self.poll;

        poll(self.future, &mut cx)
    }
}

#[cfg(test)]
mod tests {
    use crossbeam_queue::ArrayQueue;
    use std::boxed::Box;
    use std::sync::Arc;

    use crate::{common::tests::MockWaker, interrupt::yield_now};

    use super::*;

    #[test]
    fn task() {
        let queue = Arc::new(ArrayQueue::new(10));

        let test_future = |queue| {
            || {
                async fn future(queue: Arc<ArrayQueue<u32>>) {
                    for i in 0.. {
                        queue.push(i).unwrap();
                        yield_now().await;
                    }
                }

                future(queue)
            }
        };

        let mut task = Box::leak(Box::new(Task::new(test_future(queue.clone()), 1))).get_handle();
        let waker: Waker = Arc::new(MockWaker::new()).into();

        assert_eq!(task.poll(waker.clone()), Poll::Pending);

        assert_eq!(queue.pop().unwrap(), 0);
        assert!(queue.pop().is_none());

        assert_eq!(task.poll(waker.clone()), Poll::Pending);

        assert_eq!(queue.pop().unwrap(), 1);
        assert!(queue.pop().is_none());
    }
}
