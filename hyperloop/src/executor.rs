use core::cmp::Ordering;

use heapless::{Vec, binary_heap::Max};

use crate::priority_channel::Item;
use crate::waker::get_waker;

use {
    alloc::boxed::Box,
    core::{
        task::{Context, Waker, Poll},
        future::Future,
        pin::Pin,
    },
    log::error,
    crate::priority_channel::{
        channel,
        Sender,
        Receiver
    },
};

type Priority = u8;

#[derive(Debug, Clone, Copy)]
struct Ticket {
    task: *const Task,
    priority: Priority,
}

impl Item for Ticket {}

impl Ticket {
    fn new(task: *const Task, priority: Priority) -> Self {
        Self {
            task,
            priority,
        }
    }

    unsafe fn get_task(&self) -> &mut Task {
        &mut *(self.task as *mut Task)
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

type TaskSender = Sender<Ticket>;
type TaskReceiver<const N: usize> = Receiver<Ticket, Max, N>;

pub struct Task {
    future: Pin<Box<dyn Future<Output = ()> + Sync>>,
    priority: Priority,
    sender: TaskSender,
}

impl Task {
    fn new(
        future: Pin<Box<dyn Future<Output = ()> + Sync>>,
        priority: Priority,
        sender: TaskSender,
    ) -> Self {
        Self {
            future,
            priority,
            sender,
        }
    }

    fn poll(&mut self, context: &mut Context) -> Poll<()> {
        self.future.as_mut().poll(context)
    }

    pub fn get_waker(self: &Self) -> Waker {
        get_waker(self)
    }

    pub fn wake(&self) {
        self.schedule();
    }

    fn schedule(&self) {
        let ticket = Ticket::new(self as *const Self,
                                 self.priority);

        if let Err(_err) = self.sender.send(ticket) {
            error!("Failed to push to queue");
        }
    }
}

trait PushRef {
    type Item;

    fn push_ref(&mut self, item: Self::Item) -> Result<&Self::Item, Self::Item>;
}

impl<T, const N: usize> PushRef for Vec<T, N> {
    type Item = T;

    fn push_ref(&mut self, item: Self::Item) -> Result<&Self::Item, Self::Item> {
        self.push(item)?;

        Ok(self.last().unwrap())
    }
}

pub struct Executor<const N: usize> {
    tasks: Vec<Task, N>,
    sender: TaskSender,
    receiver: TaskReceiver<N>,
}

impl<const N: usize> Executor<N> {
    pub fn new() -> Self {
        let (sender, receiver) = channel();

        Self {
            tasks: Vec::new(),
            sender,
            receiver,
        }
    }

    pub fn add_task(&mut self, future: Pin<Box<dyn Future<Output = ()> + Sync>>,
                    priority: Priority) -> Result<(), ()> {
        let task = Task::new(future, priority, self.sender.clone());

        if let Ok(task) = self.tasks.push_ref(task) {
            task.schedule();
            Ok(())
        } else {
            Err(())
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
    pub unsafe fn poll_tasks(&mut self) {
        while let Ok(ticket) = self.receiver.recv() {
            let task = ticket.get_task();

            let waker = task.get_waker();
            let mut cx = Context::from_waker(&waker);

            let _ = task.poll(&mut cx);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_executor() {
        use super::*;
        use crossbeam_queue::ArrayQueue;
        use alloc::{
                sync::Arc,
                boxed::Box,
        };

        let mut executor = Executor::<10>::new();
        let queue =  Arc::new(ArrayQueue::new(10));

        async fn test_future(queue: Arc<ArrayQueue<u32>>, value: u32) {
            queue.push(value).unwrap();
        }

        executor.add_task(Box::pin(test_future(queue.clone(), 1)), 1).unwrap();
        executor.add_task(Box::pin(test_future(queue.clone(), 2)), 3).unwrap();
        executor.add_task(Box::pin(test_future(queue.clone(), 3)), 2).unwrap();
        executor.add_task(Box::pin(test_future(queue.clone(), 4)), 4).unwrap();

        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop().unwrap(), 4);
        assert_eq!(queue.pop().unwrap(), 2);
        assert_eq!(queue.pop().unwrap(), 3);
        assert_eq!(queue.pop().unwrap(), 1);
    }
}
