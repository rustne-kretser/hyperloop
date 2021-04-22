#![no_std]

extern crate alloc;

pub mod hyperloop {
    use {
        alloc::{
            collections::{BinaryHeap,BTreeMap},
            task::{Wake},
            sync::{Arc},
            boxed::Box,
        },
        core::{
            task::{Context, Waker, Poll},
            cmp::Ordering,
            future::Future,
            pin::Pin,
            sync::atomic::AtomicU32,
            sync::atomic::Ordering as AtomicOrdering,
        },
        crossbeam_queue::ArrayQueue,
        log::{error},
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    struct Priority {
        pri: u8,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub struct TaskId(u32);

    impl TaskId {
        fn new() -> Self {
            static NEXT_ID: AtomicU32 = AtomicU32::new(0);
            TaskId(NEXT_ID.fetch_add(1, AtomicOrdering::Relaxed))
        }
    }

    #[derive(Debug, Clone, Copy)]
    struct TaskTicket {
        task_id: TaskId,
        priority: Priority,
    }

    impl TaskTicket {
        fn new(task_id: TaskId, pri: u8) -> TaskTicket {
            TaskTicket {
                    task_id: task_id,
                    priority: Priority{pri},
            }
        }
    }

    impl PartialEq for TaskTicket {
        fn eq(&self, other: &Self) -> bool {
            self.priority.pri == other.priority.pri
        }
    }

    impl Eq for TaskTicket {}

    impl PartialOrd for TaskTicket {
        fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
            Some(self.cmp(other))
        }
    }

    impl Ord for TaskTicket {
        fn cmp(&self, other: &Self) -> Ordering {
            self.priority.pri.cmp(&other.priority.pri)
        }
    }

    struct TaskWaker {
        ticket: TaskTicket,
        queue: Arc<ArrayQueue<TaskTicket>>,
    }

    impl TaskWaker {
        fn new(ticket: TaskTicket, queue: Arc<ArrayQueue<TaskTicket>>) -> Waker {
            Waker::from(Arc::new(TaskWaker {
                queue,
                ticket,
            }))
        }

        fn schedule(&self) {
            if let Err(_err) = self.queue.push(self.ticket) {
                error!("Failed to push to queue");
            }
        }
    }

    impl Wake for TaskWaker {
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
        fn new(future: Pin<Box<dyn Future<Output = ()>>>, pri: u8,
               queue: Arc<ArrayQueue<TaskTicket>>) -> Self {
            let id = TaskId::new();

            Task {
                future,
                id,
                waker: TaskWaker::new(TaskTicket::new(id, pri), queue),
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

    pub struct Hyperloop {
        tasks: BTreeMap<TaskId, Task>,
        incoming: Arc<ArrayQueue<TaskTicket>>,
        pri_queue: BinaryHeap<TaskTicket>,
    }

    impl Hyperloop {
        pub fn new(capacity: usize) -> Hyperloop {
            Hyperloop {
                tasks: BTreeMap::new(),
                incoming: Arc::new(ArrayQueue::new(capacity)),
                pri_queue: BinaryHeap::new(),
            }
        }

        pub fn add_task(&mut self, future: Pin<Box<dyn Future<Output = ()>>>, pri: u8) -> TaskId {
            let task = Task::new(future, pri, self.incoming.clone());
            let task_id = task.get_id();
            self.tasks.insert(task_id, task);

            return task_id;
        }

        pub fn schedule_task(&mut self, task_id: TaskId) {
            if let Some(task) = self.tasks.get(&task_id) {
                task.schedule();
            }
        }

        fn pop_task(&mut self) -> Option<TaskTicket> {
            while let Some(ticket) = self.incoming.pop() {
                self.pri_queue.push(ticket);
            }

            self.pri_queue.pop()
        }

        pub fn poll_tasks(&mut self) {
            while let Some(ticket) = self.pop_task() {
                if let Some(task) = self.tasks.get_mut(&ticket.task_id) {
                    let waker = Waker::from(task.waker.clone());
                    let mut cx = Context::from_waker(&waker);

                    match task.poll(&mut cx) {
                        Poll::Ready(()) => {
                            self.tasks.remove(&ticket.task_id);
                        },
                        Poll::Pending => {},
                    }
                }
            }
        }
    }
}

// #[cfg(test)]
// #[macro_use]
// extern crate std;
// mod tests {
//     #[test]
//     fn test_queue() {
//         use crate::hyperloop::*;
//         use std::vec::Vec;
//         use std::sync::Mutex;

//         let hyperloop = Hyperloop::new(10);
//         let queue: Mutex<Vec = Mutex::new(Vec::new());

//         async fn fn1() {
//             queue.try_lock();
//         }
//     }
// }
