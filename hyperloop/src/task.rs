use core::{
    lazy::OnceCell,
    pin::Pin,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

use atomig::{Atom, Atomic, Ordering};
use futures::Future;

use crate::{
    executor::{Priority, TaskSender, Ticket},
    priority_queue::Sender,
};

unsafe fn clone<F: Future<Output = ()> + 'static>(ptr: *const ()) -> RawWaker {
    let task = &*(ptr as *const Task<F>);

    RawWaker::new(ptr, &task.vtable)
}

unsafe fn wake<F: Future<Output = ()> + 'static>(ptr: *const ()) {
    let task = &*(ptr as *const Task<F>);
    task.wake();
}

unsafe fn drop(_ptr: *const ()) {}

pub(crate) trait PollTask {
    unsafe fn poll(&self) -> Poll<()>;
}

#[repr(u8)]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Atom)]
enum TaskState {
    NotQueued,
    Queued,
}

pub struct Task<F>
where
    F: Future<Output = ()> + 'static,
{
    future: F,
    priority: Priority,
    sender: OnceCell<TaskSender>,
    vtable: RawWakerVTable,
    state: Atomic<TaskState>,
}

impl<F> Task<F>
where
    F: Future<Output = ()> + 'static,
{
    pub fn new(future_fn: impl FnOnce() -> F, priority: Priority) -> Self {
        Self {
            future: future_fn(),
            priority,
            sender: OnceCell::new(),
            vtable: RawWakerVTable::new(clone::<F>, wake::<F>, wake::<F>, drop),
            state: Atomic::new(TaskState::NotQueued),
        }
    }

    fn update_state(&self, old: TaskState, new: TaskState) -> bool {
        if let Ok(_) = self
            .state
            .compare_exchange(old, new, Ordering::Relaxed, Ordering::Relaxed)
        {
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    fn get_state(&self) -> TaskState {
        self.state.load(Ordering::Relaxed)
    }

    unsafe fn as_static(&self) -> &'static Self {
        &*(self as *const Self)
    }

    unsafe fn as_mut(&self) -> &mut Self {
        &mut *((self as *const Self) as *mut Self)
    }

    unsafe fn get_waker(&self) -> Waker {
        let ptr: *const () = (self as *const Task<F>).cast();
        let vtable = &self.as_static().vtable;

        Waker::from_raw(RawWaker::new(ptr, vtable))
    }

    pub fn wake(&self) {
        self.schedule().unwrap();
    }

    pub fn add_to_executor(&self, sender: TaskSender) -> Result<(), ()> {
        self.set_sender(sender)?;
        self.schedule()
    }

    fn set_sender(&self, sender: TaskSender) -> Result<(), ()> {
        match self.sender.set(sender) {
            Ok(_) => Ok(()),
            Err(_) => Err(()),
        }
    }

    fn send_ticket(&self, ticket: Ticket) -> Result<(), ()> {
        if let Some(sender) = self.sender.get() {
            if let Ok(_) = sender.send(ticket) {
                return Ok(());
            }
        }

        Err(())
    }

    fn schedule(&self) -> Result<(), ()> {
        if self.update_state(TaskState::NotQueued, TaskState::Queued) {
            let ticket = Ticket::new(self as *const Self, self.priority);

            match self.send_ticket(ticket) {
                Ok(_) => Ok(()),
                Err(_) => {
                    assert!(self.update_state(TaskState::Queued, TaskState::NotQueued));
                    Err(())
                }
            }
        } else {
            Ok(())
        }
    }
}

impl<F> PollTask for Task<F>
where
    F: Future<Output = ()> + 'static,
{
    unsafe fn poll(&self) -> Poll<()> {
        let waker = self.get_waker();
        let mut cx = Context::from_waker(&waker);
        let future = Pin::new_unchecked(&mut self.as_mut().future);

        assert!(self.update_state(TaskState::Queued, TaskState::NotQueued));
        let result = future.poll(&mut cx);

        result
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        interrupt::yield_now,
        priority_queue::{Max, PriorityQueue},
    };

    use super::*;

    #[test]
    fn task() {
        let mut queue: PriorityQueue<Ticket, Max, 1> = PriorityQueue::new();

        let test_future = || {
            || {
                async fn future() {
                    loop {
                        yield_now().await
                    }
                }

                future()
            }
        };

        let task = Task::new(test_future(), 1);

        task.set_sender(queue.get_sender()).unwrap();

        assert_eq!(task.get_state(), TaskState::NotQueued);

        task.schedule().unwrap();

        assert_eq!(task.get_state(), TaskState::Queued);

        assert!(queue.pop().is_some());
        assert!(queue.pop().is_none());

        task.schedule().unwrap();

        assert!(queue.pop().is_none());

        unsafe {
            assert_eq!(task.poll(), Poll::Pending);
        }

        assert_eq!(task.get_state(), TaskState::NotQueued);

        task.wake();
        task.wake();
        task.wake();

        assert_eq!(task.get_state(), TaskState::Queued);

        assert!(queue.pop().is_some());
        assert!(queue.pop().is_none());

        task.wake();
        task.wake();
        task.wake();

        assert_eq!(task.get_state(), TaskState::Queued);

        assert!(queue.pop().is_none());
    }
}
