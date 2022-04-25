use core::{
    cell::UnsafeCell,
    pin::Pin,
    task::{Context, Poll, Waker},
};

use embedded_time::{
    duration::{Duration, Milliseconds},
    fixed_point::FixedPoint,
    rate::{Hertz, Rate},
};

use core::future::Future;
use log::error;

use crate::priority_queue::{Min, PeekMut, PriorityQueue, PrioritySender};

type Tick = u64;

pub struct Scheduler<const N: usize> {
    rate: Hertz,
    counter: UnsafeCell<Tick>,
    queue: PriorityQueue<Ticket, Min, N>,
}

impl<const N: usize> Scheduler<N> {
    pub fn new(rate: Hertz) -> Self {
        Self {
            rate,
            counter: UnsafeCell::new(0),
            queue: PriorityQueue::new(),
        }
    }

    pub unsafe fn increment(&mut self) {
        *self.counter.get() += 1;
    }

    fn next_waker(&mut self) -> Option<Waker> {
        if let Some(ticket) = self.queue.peek_mut().as_mut() {
            if unsafe { *self.counter.get() } > ticket.expires {
                return Some(PeekMut::pop(ticket).waker);
            }
        }

        None
    }

    fn wake_tasks(&mut self) {
        while let Some(waker) = self.next_waker() {
            waker.wake();
        }
    }

    pub unsafe fn tick(&mut self) {
        self.increment();
        self.wake_tasks();
    }

    pub fn get_timer(&self) -> Timer {
        let counter = self.counter.get() as *const _;
        let sender = unsafe { self.queue.get_sender() };
        let timer = Timer::new(self.rate, counter, sender);

        timer
    }
}

#[derive(Debug, Clone)]
pub struct Ticket {
    expires: Tick,
    waker: Waker,
}

impl Ticket {
    fn new(expires: Tick, waker: Waker) -> Self {
        Self { expires, waker }
    }
}

impl PartialEq for Ticket {
    fn eq(&self, other: &Self) -> bool {
        self.expires == other.expires
    }
}

impl Eq for Ticket {}

impl PartialOrd for Ticket {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Ticket {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.expires.cmp(&other.expires)
    }
}

struct DelayFuture {
    sender: PrioritySender<Ticket>,
    counter: *const Tick,
    expires: Tick,
    started: bool,
}

impl DelayFuture {
    fn new(sender: PrioritySender<Ticket>, counter: *const Tick, expires: Tick) -> Self {
        Self {
            sender,
            counter,
            expires,
            started: false,
        }
    }
}

impl Future for DelayFuture {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.started {
            if let Err(_) = self
                .sender
                .send(Ticket::new(self.expires, cx.waker().clone()))
            {
                error!("failed to send ticket");
            }
            self.started = true;
            Poll::Pending
        } else {
            // Delay until tick count is greater than the
            // expiration. This ensures that we wait for no less than
            // the specified duration, and possibly one tick longer
            // than desired.
            if unsafe { *self.counter } > self.expires {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }
}

pub struct TimeoutFuture<F>
where
    F: Future,
{
    future: F,
    delay: DelayFuture,
}

impl<F> TimeoutFuture<F>
where
    F: Future,
{
    fn new(future: F, sender: PrioritySender<Ticket>, counter: *const Tick, expires: Tick) -> Self {
        Self {
            future,
            delay: DelayFuture::new(sender, counter, expires),
        }
    }
}

impl<F> Future for TimeoutFuture<F>
where
    F: Future,
{
    type Output = Result<F::Output, ()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let (future, delay) = unsafe {
            let this = self.get_unchecked_mut();
            (
                Pin::new_unchecked(&mut this.future),
                Pin::new_unchecked(&mut this.delay),
            )
        };

        if let Poll::Ready(ret) = future.poll(cx) {
            Poll::Ready(Ok(ret))
        } else {
            if let Poll::Ready(()) = delay.poll(cx) {
                Poll::Ready(Err(()))
            } else {
                Poll::Pending
            }
        }
    }
}

#[derive(Clone)]
pub struct Timer {
    rate: Hertz,
    counter: *const Tick,
    sender: PrioritySender<Ticket>,
}

impl Timer {
    pub fn new(rate: Hertz, counter: *const Tick, sender: PrioritySender<Ticket>) -> Self {
        Self {
            rate,
            counter,
            sender,
        }
    }

    fn get_rate(&self) -> Hertz {
        self.rate
    }

    fn get_count(&self) -> Tick {
        unsafe { *self.counter }
    }

    fn delay_to_ticks<D: Duration + Into<Milliseconds>>(&self, duration: D) -> Tick {
        let ms: Milliseconds = duration.into();
        let rate = self.get_rate().to_duration::<Milliseconds>().unwrap();

        assert!(ms.integer() == 0 || ms >= rate);

        let ticks: Tick = (ms.integer() / rate.integer()).into();

        self.get_count() + ticks
    }

    pub fn delay(&self, duration: Milliseconds) -> impl Future {
        DelayFuture::new(
            self.sender.clone(),
            self.counter,
            self.delay_to_ticks(duration),
        )
    }

    pub fn timeout<F: Future>(&self, future: F, duration: Milliseconds) -> TimeoutFuture<F> {
        TimeoutFuture::new(
            future,
            self.sender.clone(),
            self.counter,
            self.delay_to_ticks(duration),
        )
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::Ordering;

    use crate::common::tests::{log_init, MockWaker};
    use crate::executor::Executor;
    use crate::task::Task;

    use super::*;

    use core::future::Future;
    use crossbeam_queue::ArrayQueue;
    use embedded_time::duration::Extensions as Ext;
    use embedded_time::rate::Extensions;
    use std::{boxed::Box, sync::Arc};

    #[test]
    fn state() {
        let scheduler = Box::leak(Box::new(Scheduler::<0>::new(1000.Hz())));
        let timer = scheduler.get_timer();

        assert_eq!(unsafe { *timer.counter }, 0);

        unsafe { scheduler.tick() };
        assert_eq!(unsafe { *timer.counter }, 1);

        unsafe {
            scheduler.tick();
        }

        assert_eq!(unsafe { *timer.counter }, 2);
    }

    #[test]
    fn delay() {
        let scheduler = Box::leak(Box::new(Scheduler::<10>::new(1000.Hz())));
        let timer = scheduler.get_timer();
        let counter = timer.counter;

        let mockwaker = Arc::new(MockWaker::new());
        let waker: Waker = mockwaker.clone().into();
        let mut cx = Context::from_waker(&waker);

        let mut future = DelayFuture::new(timer.sender.clone(), counter, 1);

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);
        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);
        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);
        assert_eq!(future.started, true);

        unsafe {
            scheduler.increment();
            scheduler.increment();
        }

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Ready(()));

        let mut future = DelayFuture::new(timer.sender.clone(), counter, 20);

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);

        let mut future = DelayFuture::new(timer.sender.clone(), counter, 15);

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);

        if let Some(ticket) = scheduler.queue.pop() {
            assert_eq!(ticket.expires, 1);
            ticket.waker.wake();
            assert_eq!(mockwaker.woke.load(Ordering::Relaxed), true)
        }

        if let Some(ticket) = scheduler.queue.pop() {
            assert_eq!(ticket.expires, 15);
        }

        if let Some(ticket) = scheduler.queue.pop() {
            assert_eq!(ticket.expires, 20);
        }
    }

    #[test]
    fn timer() {
        let scheduler = Box::leak(Box::new(Scheduler::new(1000.Hz())));
        let timer = scheduler.get_timer();

        log_init();

        let test_future = |queue, timer| {
            move || {
                async fn future(queue: Arc<ArrayQueue<u32>>, timer: Timer) {
                    queue.push(1).unwrap();

                    timer.delay(0.milliseconds()).await;

                    queue.push(2).unwrap();

                    timer.delay(1.milliseconds()).await;

                    queue.push(3).unwrap();

                    timer.delay(1.milliseconds()).await;

                    queue.push(4).unwrap();

                    timer.delay(1.milliseconds()).await;

                    queue.push(5).unwrap();

                    timer.delay(10.milliseconds()).await;

                    queue.push(6).unwrap();
                }

                future(queue, timer)
            }
        };

        let queue = Arc::new(ArrayQueue::new(10));

        let task = Box::leak(Box::new(Task::new(
            test_future(queue.clone(), timer.clone()),
            1,
        )))
        .get_handle();

        let mut executor = Box::leak(Box::new(Executor::new([task])))
            .get_handle()
            .with_scheduler(scheduler);

        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), None);

        unsafe {
            scheduler.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);

        scheduler.wake_tasks();
        executor.poll_tasks();

        assert_eq!(queue.pop(), None);

        unsafe {
            scheduler.tick();
        }
        unsafe {
            scheduler.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), None);

        unsafe {
            scheduler.tick();
        }
        unsafe {
            scheduler.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(4));
        assert_eq!(queue.pop(), None);

        unsafe {
            scheduler.tick();
        }
        unsafe {
            scheduler.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(5));
        assert_eq!(queue.pop(), None);

        for _ in 0..10 {
            unsafe {
                scheduler.tick();
            }
            executor.poll_tasks();

            assert_eq!(queue.pop(), None);
        }

        unsafe {
            scheduler.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(6));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn timeout() {
        let scheduler = Box::leak(Box::new(Scheduler::<2>::new(1000.Hz())));
        let timer = scheduler.get_timer();

        log_init();

        let waiting_future = |queue, timer| {
            move || {
                async fn slow_future(timer: Timer) {
                    timer.delay(1000.milliseconds()).await;
                }

                async fn future(queue: Arc<ArrayQueue<u32>>, timer: Timer) {
                    queue.push(1).unwrap();

                    assert_eq!(
                        timer
                            .timeout(slow_future(timer.clone()), 100.milliseconds())
                            .await,
                        Err(())
                    );
                    queue.push(2).unwrap();

                    assert_eq!(
                        timer
                            .timeout(slow_future(timer.clone()), 1001.milliseconds())
                            .await,
                        Ok(())
                    );
                    queue.push(3).unwrap();
                }
                future(queue, timer)
            }
        };

        let queue = Arc::new(ArrayQueue::new(10));

        let task =
            Box::leak(Box::new(Task::new(waiting_future(queue.clone(), timer), 1))).get_handle();

        let mut executor = Box::leak(Box::new(Executor::new([task]))).get_handle();

        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), None);

        for _ in 0..101 {
            unsafe {
                scheduler.increment();
            }
        }

        scheduler.wake_tasks();

        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);

        for _ in 0..1000 {
            unsafe {
                scheduler.increment();
            }
            scheduler.wake_tasks();

            executor.poll_tasks();
            assert_eq!(queue.pop(), None);
        }

        unsafe {
            scheduler.increment();
        }
        scheduler.wake_tasks();

        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), None);
    }
}
