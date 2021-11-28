#![no_std]

extern crate alloc;

mod priority_channel;
pub mod timer;
pub mod notify;

pub mod hyperloop {
    use core::{cmp::Ordering, sync::atomic::AtomicU8};

    use heapless::binary_heap::Max;

    use crate::priority_channel::Item;

    use {
        alloc::{
            collections::BTreeMap,
            task::Wake,
            sync::Arc,
            boxed::Box,
        },
        core::{
            task::{Context, Waker, Poll},
            future::Future,
            pin::Pin,
            sync::atomic::Ordering as AtomicOrdering,
        },
        log::error,
        crate::priority_channel::{
            channel,
            Sender,
            Receiver
        },
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub struct TaskId(u8);

    impl TaskId {
        fn new() -> Self {
            static NEXT_ID: AtomicU8 = AtomicU8::new(0);
            TaskId(NEXT_ID.fetch_add(1, AtomicOrdering::Relaxed))
        }
    }

    pub trait Priority: 'static + Ord + Sync + Send + Copy {}

    impl Priority for u8 {}

    #[derive(Debug, Clone, Copy)]
    struct Ticket<P>
    where P: Priority {
        task: TaskId,
        priority: P,
    }

    impl<P> Item for Ticket<P>
    where P: Priority {}

    impl<P> Ticket<P>
    where P: Priority {
        fn new(task: TaskId, priority: P) -> Self {
            Self {
                task,
                priority,
            }
        }
    }

    impl<P> PartialEq for Ticket<P>
    where P: Priority {
        fn eq(&self, other: &Self) -> bool {
            self.priority == other.priority
        }
    }

    impl<P> Eq for Ticket<P>
    where P: Priority {}

    impl<P> PartialOrd for Ticket<P>
    where P: Priority {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    impl<P> Ord for Ticket<P>
    where P: Priority {
        fn cmp(&self, other: &Self) -> Ordering {
            self.priority.cmp(&other.priority)
        }
    }

    type TaskSender<P> = Sender<Ticket<P>>;
    type TaskReceiver<P, const N: usize> = Receiver<Ticket<P>, Max, N>;

    #[derive(Clone)]
    struct TaskHandle<P>
    where P: Priority {
        task_id: TaskId,
        priority: P,
        sender: TaskSender<P>,
    }

    impl<P> TaskHandle<P>
    where P: Priority {
        fn new(task_id: TaskId, priority: P,
               sender: TaskSender<P>) -> Self {
            Self {
                task_id,
                priority,
                sender,
            }
        }

        fn get_waker(self: Arc<Self>) -> Waker {
            Waker::from(self)
        }

        fn schedule(&self) {
            let ticket = Ticket::new(self.task_id, self.priority);

            if let Err(_err) = self.sender.send(ticket) {
                error!("Failed to push to queue");
            }
        }
    }

    impl<P> Wake for TaskHandle<P>
    where P: Priority {
        fn wake(self: Arc<Self>) {
            self.schedule();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.schedule();
        }
    }

    pub struct Task<P>
    where P: Priority {
        future: Pin<Box<dyn Future<Output = ()> + Sync>>,
        id: TaskId,
        handle: Arc<TaskHandle<P>>,
    }

    impl<P> Task<P>
    where P: Priority {
        fn new(
            future: Pin<Box<dyn Future<Output = ()> + Sync>>,
            pri: P,
            sender: TaskSender<P>,
        ) -> Self {
            let id = TaskId::new();

            Self {
                future,
                id,
                handle: Arc::new(TaskHandle::new(id, pri, sender)),
            }
        }

        fn schedule(&self) {
            self.handle.schedule();
        }

        fn poll(&mut self, context: &mut Context) -> Poll<()> {
            self.future.as_mut().poll(context)
        }

        fn get_id(&self) -> TaskId {
            self.id
        }
    }

    pub struct Hyperloop<P, const N: usize>
    where P: Priority {
        tasks: BTreeMap<TaskId, Task<P>>,
        sender: TaskSender<P>,
        receiver: TaskReceiver<P, N>,
    }

    impl<P, const N: usize> Hyperloop<P, N>
    where P: Priority {
        pub fn new() -> Self {
            let (sender, receiver) = channel();

            Self {
                tasks: BTreeMap::new(),
                sender,
                receiver,
            }
        }

        pub fn add_task(&mut self, future: Pin<Box<dyn Future<Output = ()> + Sync>>,
                        pri: P) -> TaskId {
            let task = Task::new(future, pri, self.sender.clone());
            let task_id = task.get_id();
            self.tasks.insert(task_id, task);

            task_id
        }

        pub fn schedule_task(&mut self, task_id: TaskId) {
            if let Some(task) = self.tasks.get(&task_id) {
                task.schedule();
            }
        }

        pub fn add_and_schedule(&mut self,
                                future: Pin<Box<dyn Future<Output = ()> + Sync>>,
                                pri: P) -> TaskId {
            let id = self.add_task(future, pri);
            self.schedule_task(id);
            id
        }

        pub fn poll_tasks(&mut self) {
            while let Ok(ticket) = self.receiver.recv() {
                if let Some(task) = self.tasks.get_mut(&ticket.task) {
                    let waker = task.handle.clone().get_waker();
                    let mut cx = Context::from_waker(&waker);

                    match task.poll(&mut cx) {
                        Poll::Ready(()) => {
                            self.tasks.remove(&ticket.task);
                        },
                        Poll::Pending => {},
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[macro_use]
extern crate std;
mod tests {
    #[test]
    fn test_executor() {
        use crate::hyperloop::*;
        use crossbeam_queue::ArrayQueue;
        use alloc::{
                sync::Arc,
                boxed::Box,
        };

        let mut hyperloop = Hyperloop::<_, 10>::new();
        let queue =  Arc::new(ArrayQueue::new(10));

        async fn test_future(queue: Arc<ArrayQueue<u32>>, value: u32) {
            queue.push(value).unwrap();
        }

        hyperloop.add_and_schedule(Box::pin(test_future(queue.clone(), 1)), 1);
        hyperloop.add_and_schedule(Box::pin(test_future(queue.clone(), 2)), 3);
        hyperloop.add_and_schedule(Box::pin(test_future(queue.clone(), 3)), 2);
        hyperloop.add_and_schedule(Box::pin(test_future(queue.clone(), 4)), 4);

        hyperloop.poll_tasks();

        assert_eq!(queue.pop().unwrap(), 4);
        assert_eq!(queue.pop().unwrap(), 2);
        assert_eq!(queue.pop().unwrap(), 3);
        assert_eq!(queue.pop().unwrap(), 1);
    }
}
