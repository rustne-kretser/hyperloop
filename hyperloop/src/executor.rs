use core::cmp::Ordering;

use crate::priority_queue::{Max, PriorityQueue, PrioritySender};

use crate::task::PollTask;

pub(crate) type Priority = u8;

#[derive(Debug, Clone, Copy)]
pub struct Ticket {
    task: *const dyn PollTask,
    priority: Priority,
}

impl Ticket {
    pub(crate) fn new(task: *const dyn PollTask, priority: Priority) -> Self {
        Self { task, priority }
    }

    unsafe fn get_task(&self) -> &dyn PollTask {
        &*self.task
    }
}

impl PartialEq for Ticket {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority
    }
}

impl Eq for Ticket {}

impl PartialOrd for Ticket {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Ticket {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority.cmp(&other.priority)
    }
}

pub(crate) type TaskSender = PrioritySender<Ticket>;

pub struct Executor<const N: usize> {
    queue: PriorityQueue<Ticket, Max, N>,
}

impl<const N: usize> Executor<N> {
    pub fn new() -> Self {
        Self {
            queue: PriorityQueue::new(),
        }
    }

    /// Poll all tasks in the queue
    ///
    /// # Safety
    ///
    /// This function is unsafe. The caller must guarantee that the
    /// executor is never dropped or moved. The wakers contain raw
    /// pointers to the tasks stored in the executor. The pointers can
    /// be dereferenced at any time and will be dangling if the
    /// exeutor is moved or dropped.
    unsafe fn poll_tasks(&mut self) {
        while let Some(ticket) = self.queue.pop() {
            let _ = ticket.get_task().poll();
        }
    }

    unsafe fn get_sender(&self) -> TaskSender {
        self.queue.get_sender()
    }

    pub fn get_ref(&'static mut self) -> ExecutorRef<N> {
        ExecutorRef::new(self)
    }
}

/// Wrapper around Executor to allow safe polling
pub struct ExecutorRef<const N: usize> {
    executor: &'static mut Executor<N>,
}

impl<const N: usize> ExecutorRef<N> {
    fn new(executor: &'static mut Executor<N>) -> Self {
        Self { executor }
    }

    pub fn poll_tasks(&mut self) {
        unsafe {
            self.executor.poll_tasks();
        }
    }

    pub fn get_sender(&self) -> TaskSender {
        unsafe { self.executor.get_sender() }
    }
}

#[cfg(test)]
mod tests {
    use crossbeam_queue::ArrayQueue;
    use std::boxed::Box;
    use std::sync::Arc;

    use super::*;
    use crate::task::Task;

    #[test]
    fn test_executor() {
        let mut executor = Box::leak(Box::new(Executor::<10>::new())).get_ref();
        let queue = Arc::new(ArrayQueue::new(10));

        let test_future = |queue, value| {
            move || {
                async fn future(queue: Arc<ArrayQueue<u32>>, value: u32) {
                    queue.push(value).unwrap();
                }

                future(queue, value)
            }
        };

        let task1 = Task::new(test_future(queue.clone(), 1), 1);
        let task2 = Task::new(test_future(queue.clone(), 2), 3);
        let task3 = Task::new(test_future(queue.clone(), 3), 2);
        let task4 = Task::new(test_future(queue.clone(), 4), 4);

        task1.add_to_executor(executor.get_sender()).unwrap();
        task2.add_to_executor(executor.get_sender()).unwrap();
        task3.add_to_executor(executor.get_sender()).unwrap();
        task4.add_to_executor(executor.get_sender()).unwrap();

        executor.poll_tasks();

        assert_eq!(queue.pop().unwrap(), 4);
        assert_eq!(queue.pop().unwrap(), 2);
        assert_eq!(queue.pop().unwrap(), 3);
        assert_eq!(queue.pop().unwrap(), 1);
    }
}
