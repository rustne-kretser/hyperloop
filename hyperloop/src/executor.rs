use core::cmp::Ordering;
use core::sync::atomic::AtomicBool;
use core::task::{Poll, RawWaker, RawWakerVTable, Waker};

use crate::priority_queue::{Max, PriorityQueue, PrioritySender};

use crate::task::TaskHandle;

pub(crate) type Priority = u8;
type TaskId = u16;

pub struct Ticket {
    task: TaskId,
    priority: Priority,
}

impl Ticket {
    pub(crate) fn new(task: TaskId, priority: Priority) -> Self {
        Self { task, priority }
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

const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake, drop);

unsafe fn clone(ptr: *const ()) -> RawWaker {
    RawWaker::new(ptr, &VTABLE)
}

unsafe fn wake(ptr: *const ()) {
    let task = &*(ptr as *const ExecutorTask);
    task.wake();
}

unsafe fn drop(_ptr: *const ()) {}

struct ExecutorTask {
    task: TaskHandle,
    task_id: TaskId,
    priority: Priority,
    sender: Option<TaskSender>,
    pending_wake: AtomicBool,
}

impl ExecutorTask {
    fn new(
        task: TaskHandle,
        task_id: TaskId,
        priority: Priority,
        sender: Option<TaskSender>,
    ) -> Self {
        Self {
            task,
            task_id,
            priority,
            sender,
            pending_wake: AtomicBool::new(false),
        }
    }

    fn set_sender(&mut self, sender: TaskSender) {
        self.sender = Some(sender);
    }

    fn get_waker(&self) -> Waker {
        let ptr: *const () = (self as *const ExecutorTask).cast();
        let vtable = &VTABLE;

        unsafe { Waker::from_raw(RawWaker::new(ptr, vtable)) }
    }

    fn send_ticket(&self, ticket: Ticket) -> Result<(), ()> {
        let sender = unsafe { self.sender.as_ref().unwrap_unchecked() };

        sender.send(ticket).map_err(|_| ())
    }

    fn wake(&self) {
        if let Ok(_) = self.pending_wake.compare_exchange(
            false,
            true,
            atomig::Ordering::Acquire,
            atomig::Ordering::Acquire,
        ) {
            let ticket = Ticket::new(self.task_id, self.priority);

            self.send_ticket(ticket).unwrap_or_else(|_| unreachable!());
        }
    }

    fn clear_pending_wake_flag(&self) {
        let _ = self.pending_wake.compare_exchange(
            true,
            false,
            atomig::Ordering::Acquire,
            atomig::Ordering::Acquire,
        );
    }

    fn poll(&mut self, waker: Waker) -> Poll<()> {
        self.task.poll(waker)
    }
}

pub struct Executor<const N: usize> {
    tasks: [ExecutorTask; N],
    queue: PriorityQueue<Ticket, Max, N>,
}

impl<const N: usize> Executor<N> {
    pub fn new(tasks: [TaskHandle; N]) -> Self {
        let mut i = 0;
        let tasks = tasks.map(|task| {
            let priority = task.priority;
            let task = ExecutorTask::new(task, i, priority, None);
            i += 1;
            task
        });

        Self {
            tasks,
            queue: PriorityQueue::new(),
        }
    }

    unsafe fn get_task(&mut self, task_id: TaskId) -> &mut ExecutorTask {
        let index = task_id as usize;

        let task = &mut self.tasks[index];
        task.clear_pending_wake_flag();

        task
    }

    unsafe fn init(&mut self) {
        for i in 0..N {
            let sender = self.queue.get_sender();
            let task = self.get_task(i as u16);

            task.set_sender(sender);
            task.wake();
        }
    }

    unsafe fn poll_task(&mut self, task_id: TaskId) {
        let task = self.get_task(task_id);
        let waker = task.get_waker();

        let _ = task.poll(waker);
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
            self.poll_task(ticket.task);
        }
    }

    pub fn get_handle(&'static mut self) -> ExecutorHandle<N> {
        ExecutorHandle::new(self)
    }
}

pub struct ExecutorHandle<const N: usize> {
    executor: &'static mut Executor<N>,
}

impl<const N: usize> ExecutorHandle<N> {
    pub fn new(executor: &'static mut Executor<N>) -> Self {
        unsafe { executor.init() };
        Self { executor }
    }

    /// Poll all tasks in the queue
    pub fn poll_tasks(&mut self) {
        unsafe { self.executor.poll_tasks() }
    }
}

#[cfg(test)]
mod tests {
    use crossbeam_queue::ArrayQueue;
    use hyperloop_macros::task;
    use std::boxed::Box;
    use std::sync::Arc;

    use super::*;
    use crate::notify::Notification;
    use crate::task::Task;

    #[test]
    fn test_executor() {
        let queue = Arc::new(ArrayQueue::new(10));

        let test_future = |queue, value| {
            move || {
                async fn future(queue: Arc<ArrayQueue<u32>>, value: u32) {
                    queue.push(value).unwrap();
                }

                future(queue, value)
            }
        };

        let task1 = Box::leak(Box::new(Task::new(test_future(queue.clone(), 1), 1)));
        let task2 = Box::leak(Box::new(Task::new(test_future(queue.clone(), 2), 3)));
        let task3 = Box::leak(Box::new(Task::new(test_future(queue.clone(), 3), 2)));
        let task4 = Box::leak(Box::new(Task::new(test_future(queue.clone(), 4), 4)));

        let mut executor = ExecutorHandle::new(Box::leak(Box::new(Executor::new([
            task1.get_handle(),
            task2.get_handle(),
            task3.get_handle(),
            task4.get_handle(),
        ]))));

        executor.poll_tasks();

        assert_eq!(queue.pop().unwrap(), 4);
        assert_eq!(queue.pop().unwrap(), 2);
        assert_eq!(queue.pop().unwrap(), 3);
        assert_eq!(queue.pop().unwrap(), 1);
    }

    #[test]
    fn test_pending_wake() {
        let queue = Arc::new(ArrayQueue::new(10));
        let notify = Box::leak(Box::new(Notification::new()));

        let test_future = |queue, notify| {
            move || {
                async fn future(queue: Arc<ArrayQueue<u32>>, notify: &'static Notification) {
                    for i in 0..10 {
                        queue.push(i).unwrap();
                        notify.wait().await;
                    }
                }

                future(queue, notify)
            }
        };

        let task = Box::leak(Box::new(Task::new(test_future(queue.clone(), notify), 1)));

        let mut executor =
            ExecutorHandle::new(Box::leak(Box::new(Executor::new([task.get_handle()]))));

        executor.poll_tasks();

        assert_eq!(queue.pop().unwrap(), 0);
        assert!(queue.pop().is_none());

        notify.notify();

        executor.poll_tasks();

        assert_eq!(queue.pop().unwrap(), 1);
        assert!(queue.pop().is_none());

        executor.poll_tasks();
        assert!(queue.pop().is_none());

        let waker = unsafe { executor.executor.get_task(0).get_waker() };

        waker.wake();

        notify.notify();
        executor.poll_tasks();

        assert_eq!(queue.pop().unwrap(), 2);
        assert!(queue.pop().is_none());
    }

    #[test]
    fn macros() {
        #[task(priority = 1)]
        async fn test_task1(queue: Arc<ArrayQueue<u32>>) {
            queue.push(1).unwrap();
        }

        #[task(priority = 2)]
        async fn test_task2(queue: Arc<ArrayQueue<u32>>) {
            queue.push(2).unwrap();
        }

        let queue = Arc::new(ArrayQueue::new(10));

        let task1 = test_task1(queue.clone()).unwrap();
        let task2 = test_task2(queue.clone()).unwrap();

        let mut executor = ExecutorHandle::new(Box::leak(Box::new(Executor::new([task1, task2]))));

        executor.poll_tasks();

        assert_eq!(queue.pop().unwrap(), 2);
        assert_eq!(queue.pop().unwrap(), 1);
    }
}
