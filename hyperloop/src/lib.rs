#![no_std]

extern crate alloc;

pub mod priority_channel {
    use {
        alloc::{
            collections::{BinaryHeap},
            sync::{Arc},
        },
        core::{
            cmp::Ordering,
            fmt,
        },
        crossbeam_queue::ArrayQueue,
    };

    #[derive(Debug, Clone, Copy)]
    struct Ticket<T, P>
    where T: Copy,
          P: Ord + Copy {
        item: T,
        priority: P,
    }

    impl<T, P> Ticket<T, P>
    where T: Copy,
          P: Ord + Copy {
        fn new(item: T, priority: P) -> Self {
            Self { item, priority }
        }
    }

    impl<T: Copy, P: Ord + Copy> PartialEq for Ticket<T, P> {
        fn eq(&self, other: &Self) -> bool {
            self.priority == other.priority
        }
    }

    impl<T: Copy, P: Ord + Copy> Eq for Ticket<T, P> {}

    impl<T: Copy, P: Ord + Copy> PartialOrd for Ticket<T, P> {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    impl<T: Copy, P: Ord + Copy> Ord for Ticket<T, P> {
        fn cmp(&self, other: &Self) -> Ordering {
            self.priority.cmp(&other.priority)
        }
    }

    struct Channel<T, P>
    where T: Copy,
          P: Ord + Copy {
        in_queue: Arc<ArrayQueue<Ticket<T, P>>>,
        out_queue: BinaryHeap<Ticket<T, P>>,
    }

    impl<T, P> Channel<T, P>
    where T: Copy,
          P: Ord + Copy {
        fn new(capacity: usize) -> Self {
            Self {
                in_queue: Arc::new(ArrayQueue::new(capacity)),
                out_queue: BinaryHeap::new(),
            }
        }
    }

    #[derive(Debug, Clone)]
    pub struct Sender<T, P>
    where T: Copy,
          P: Ord + Copy {
        queue: Arc<ArrayQueue<Ticket<T, P>>>
    }

    impl<T, P> Sender<T, P>
    where T: Copy,
          P: Ord + Copy {
        fn new(queue: Arc<ArrayQueue<Ticket<T, P>>>) -> Self {
            Self {
                queue
            }
        }

        pub fn send(&self, item: T, priority: P) -> Result<(), T> {
            if let Err(ticket) = self.queue.push(Ticket::new(item, priority)) {
                Err(ticket.item)
            } else {
                Ok(())
            }
        }
    }

    #[derive(Debug, PartialEq)]
    pub struct RecvError {}

    impl fmt::Display for RecvError {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "failed to receive item!")
        }
    }


    pub struct Receiver<T, P>
    where T: Copy,
          P: Ord + Copy {
        channel: Channel<T, P>,
    }

    impl<T, P> Receiver<T, P>
    where T: Copy,
          P: Ord + Copy {
        fn new(channel: Channel<T, P>) -> Self {
            Self {
                channel
            }
        }

        pub fn recv(&mut self) -> Result<T, RecvError> {
            while let Some(ticket) = self.channel.in_queue.pop() {
                self.channel.out_queue.push(ticket);
            }

            if let Some(ticket) = self.channel.out_queue.pop() {
                Ok(ticket.item)
            } else {
                Err(RecvError {})
            }

        }
    }

    pub fn channel<T, P>(capacity: usize) -> (Sender<T, P>, Receiver<T, P>)
    where T: Copy,
          P: Ord + Copy {
        let pri_queue = Channel::new(capacity);
        (Sender::new(pri_queue.in_queue.clone()),
         Receiver::new(pri_queue))
    }
}

pub mod hyperloop {
    use {
        alloc::{
            collections::{BTreeMap},
            task::{Wake},
            sync::{Arc},
            boxed::Box,
        },
        core::{
            task::{Context, Waker, Poll},
            future::Future,
            pin::Pin,
            sync::atomic::AtomicU32,
            sync::atomic::Ordering as AtomicOrdering,
        },
        log::{error},
        crate::priority_channel::{
            channel,
            Sender,
            Receiver
        },
    };


    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub struct TaskId(u32);

    impl TaskId {
        fn new() -> Self {
            static NEXT_ID: AtomicU32 = AtomicU32::new(0);
            TaskId(NEXT_ID.fetch_add(1, AtomicOrdering::Relaxed))
        }
    }

    type TaskSender<P> = Sender<TaskId, P>;
    type TaskReceiver<P> = Receiver<TaskId, P>;

    struct TaskWaker<P: Ord + Copy> {
        task_id: TaskId,
        priority: P,
        sender: TaskSender<P>,
    }

    impl<P: 'static + Ord + Sync + Send + Copy> TaskWaker<P> {
        fn get_waker(task_id: TaskId, priority: P, sender: TaskSender<P>) -> Waker {
            Waker::from(Arc::new(TaskWaker {
                task_id,
                priority,
                sender,
            }))
        }

        fn schedule(&self) {
            if let Err(_err) = self.sender.send(self.task_id, self.priority) {
                error!("Failed to push to queue");
            }
        }
    }

    impl<P: 'static + Ord + Sync + Send + Copy> Wake for TaskWaker<P> {
        fn wake(self: Arc<Self>) {
            self.schedule();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.schedule();
        }
    }

    pub struct Task {
        future: Pin<Box<dyn Future<Output = ()>>>,
        id: TaskId,
        waker: Waker,
    }

    impl Task {
        fn new<P: 'static + Ord + Sync + Send + Copy>(future: Pin<Box<dyn Future<Output = ()>>>,
               pri: P,
               sender: TaskSender<P>) -> Self {
            let id = TaskId::new();

            Task {
                future,
                id,
                waker: TaskWaker::get_waker(id, pri, sender),
            }
        }

        fn schedule(&self) {
            self.waker.wake_by_ref();
        }

        fn poll(&mut self, context: &mut Context) -> Poll<()> {
            self.future.as_mut().poll(context)
        }

        fn get_id(&self) -> TaskId {
            self.id
        }
    }

    pub struct Hyperloop<P: Ord + Copy> {
        tasks: BTreeMap<TaskId, Task>,
        sender: TaskSender<P>,
        receiver: TaskReceiver<P>,
    }

    impl<P: 'static + Ord + Sync + Send + Copy> Hyperloop<P> {
        pub fn new(capacity: usize) -> Self {
            let (sender, receiver) = channel(capacity);

            Self {
                tasks: BTreeMap::new(),
                sender, receiver,
            }
        }

        pub fn add_task(&mut self, future: Pin<Box<dyn Future<Output = ()>>>,
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
                                future: Pin<Box<dyn Future<Output = ()>>>,
                                pri: P) -> TaskId {
            let id = self.add_task(future, pri);
            self.schedule_task(id);
            id
        }

        pub fn poll_tasks(&mut self) {
            while let Ok(task_id) = self.receiver.recv() {
                if let Some(task) = self.tasks.get_mut(&task_id) {
                    let waker = task.waker.clone();
                    let mut cx = Context::from_waker(&waker);

                    match task.poll(&mut cx) {
                        Poll::Ready(()) => {
                            self.tasks.remove(&task_id);
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
    fn test_priority_channel() {
        use crate::priority_channel::*;

        let (sender, mut receiver) = channel(5);

        sender.send(2, 2).unwrap();
        sender.send(4, 4).unwrap();
        sender.send(1, 1).unwrap();
        sender.send(3, 3).unwrap();
        sender.send(5, 5).unwrap();

        assert_eq!(sender.send(6, 6), Err(6));

        assert_eq!(receiver.recv(), Ok(5));
        assert_eq!(receiver.recv(), Ok(4));
        assert_eq!(receiver.recv(), Ok(3));
        assert_eq!(receiver.recv(), Ok(2));
        assert_eq!(receiver.recv(), Ok(1));
        assert_eq!(receiver.recv(), Err(RecvError {} ));
    }

    #[test]
    fn test_executor() {
        use crate::hyperloop::*;
        use crossbeam_queue::ArrayQueue;
        use {
            alloc::{
                sync::{Arc},
                boxed::{Box},
            },
        };

        let mut hyperloop = Hyperloop::new(10);
        let queue =  Arc::new(ArrayQueue::new(10));

        async fn test_future(queue: Arc<ArrayQueue<u32>>, value: u32) {
            if let Err(_err) = queue.push(value) {
                println!("Failed to push!");
            }
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
