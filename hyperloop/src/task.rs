use core::{lazy::OnceCell, pin::Pin, task::{Context, Poll, RawWaker, RawWakerVTable, Waker}};

use futures::Future;
use log::error;

use crate::{executor::{Priority, Ticket}, priority_queue::Sender};

unsafe fn clone<F, S>(ptr: *const ()) -> RawWaker
where F: Future<Output = ()> + 'static,
      S: Sender<Item = Ticket> + 'static {
    let task = &*(ptr as *const Task<F, S>);

    RawWaker::new(ptr, &task.vtable)
}

unsafe fn wake<F, S>(ptr: *const ())
where F: Future<Output = ()> + 'static,
    S: Sender<Item = Ticket> + 'static {
        let task = &*(ptr as *const Task<F, S>);
    task.wake();
}

unsafe fn drop(_ptr: *const ()) {
}

pub(crate) trait PollTask {
    unsafe fn poll(&self) -> Poll<()>;
}

pub struct Task<F, S>
where F: Future<Output = ()> + 'static,
      S: Sender<Item = Ticket> {
    future: F,
    priority: Priority,
    sender: OnceCell<S>,
    vtable: RawWakerVTable,
}

impl<F, S> Task<F, S>
where F: Future<Output = ()> + 'static,
      S: Sender<Item = Ticket> + 'static {
    pub fn new(
        future_fn: impl FnOnce() -> F,
        priority: Priority,
    ) -> Self {
        Self {
            future: future_fn(),
            priority,
            sender: OnceCell::new(),
            vtable: RawWakerVTable::new(clone::<F, S>,
                                        wake::<F, S>,
                                        wake::<F, S>,
                                        drop),
        }
    }

    unsafe fn as_static(&self) -> &'static Self {
        &*(self as *const Self)
    }

    unsafe fn as_mut(&self) -> &mut Self {
        &mut *((self as *const Self) as *mut Self)
    }

    unsafe fn get_waker(&self) -> Waker {
        let ptr: *const () = (self as *const Task<F, S>).cast();
        let vtable = &self.as_static().vtable;

        Waker::from_raw(RawWaker::new(ptr, vtable))
    }

    pub fn wake(&self) {
        self.schedule().unwrap();
    }

    pub fn add_to_executor(&self, sender: S) -> Result<(), ()> {
        self.set_sender(sender)?;
        self.schedule()
    }

    fn set_sender(&self, sender: S) -> Result<(), ()> {
        match self.sender.set(sender) {
            Ok(_) => Ok(()),
            Err(_) => Err(()),
        }
    }

    fn schedule(&self) -> Result<(), ()> {
        let ticket = Ticket::new(self as *const Self,
                                 self.priority);

        if let Some(sender) = self.sender.get() {
            if let Err(_err) = sender.send(ticket) {
                error!("Failed to push to queue");
            }

            Ok(())
        } else {
            Err(())
        }
    }
}

impl<F, S> PollTask for Task<F, S>
where F: Future<Output = ()> + 'static,
      S: Sender<Item = Ticket> + 'static {
    unsafe fn poll(&self) -> Poll<()> {
        let waker = self.get_waker();
        let mut cx = Context::from_waker(&waker);
        let future = Pin::new_unchecked(&mut self.as_mut().future);

        future.poll(&mut cx)
    }
}
