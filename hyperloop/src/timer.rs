use core::{
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
    counter: Tick,
    queue: PriorityQueue<Ticket, Min, N>,
}

impl<const N: usize> Scheduler<N> {
    pub fn new(rate: Hertz) -> Self {
        Self {
            rate,
            counter: 0,
            queue: PriorityQueue::new(),
        }
    }

    pub unsafe fn increment(&mut self) {
        self.counter += 1;
    }

    fn next_waker(&mut self) -> Option<Waker> {
        if let Some(ticket) = self.queue.peek_mut().as_mut() {
            if self.counter > ticket.expires {
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

    pub fn split(&'static mut self) -> (Ticker<N>, Timer) {
        let counter: &'static Tick = unsafe { &*(&self.counter as *const Tick) };
        let sender = unsafe { self.queue.get_sender() };
        let timer = Timer::new(self.rate, TickReader::new(counter), sender);
        let ticker = Ticker::new(self);

        (ticker, timer)
    }
}

#[derive(Clone)]
pub struct TickReader {
    counter: &'static Tick,
}

impl TickReader {
    fn new(counter: &'static Tick) -> Self {
        Self { counter }
    }

    fn get_count(&self) -> Tick {
        *self.counter
    }
}

pub struct Ticker<const N: usize> {
    scheduler: &'static mut Scheduler<N>,
}

impl<const N: usize> Ticker<N> {
    pub fn new(scheduler: &'static mut Scheduler<N>) -> Self {
        Self { scheduler }
    }

    pub unsafe fn tick(&mut self) {
        self.scheduler.increment();
        self.scheduler.wake_tasks();
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
    counter: TickReader,
    expires: Tick,
    started: bool,
}

impl DelayFuture {
    fn new(sender: PrioritySender<Ticket>, counter: TickReader, expires: Tick) -> Self {
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
            if self.counter.get_count() > self.expires {
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
    fn new(future: F, sender: PrioritySender<Ticket>, counter: TickReader, expires: Tick) -> Self {
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
    counter: TickReader,
    sender: PrioritySender<Ticket>,
}

impl Timer {
    pub fn new(rate: Hertz, counter: TickReader, sender: PrioritySender<Ticket>) -> Self {
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
        self.counter.get_count()
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
            self.counter.clone(),
            self.delay_to_ticks(duration),
        )
    }

    pub fn timeout<F: Future>(&self, future: F, duration: Milliseconds) -> TimeoutFuture<F> {
        TimeoutFuture::new(
            future,
            self.sender.clone(),
            self.counter.clone(),
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
        let (mut ticker, timer) = Box::leak(Box::new(Scheduler::<0>::new(1000.Hz()))).split();
        let counter = timer.counter;

        assert_eq!(counter.get_count(), 0);

        unsafe { ticker.tick() };
        assert_eq!(counter.get_count(), 1);

        unsafe {
            ticker.tick();
        }

        assert_eq!(counter.get_count(), 2);
    }

    #[test]
    fn delay() {
        let (ticker, timer) = Box::leak(Box::new(Scheduler::<10>::new(1000.Hz()))).split();
        let counter = timer.counter.clone();

        let mockwaker = Arc::new(MockWaker::new());
        let waker: Waker = mockwaker.clone().into();
        let mut cx = Context::from_waker(&waker);

        let mut future = DelayFuture::new(timer.sender.clone(), counter.clone(), 1);

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);
        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);
        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);
        assert_eq!(future.started, true);

        unsafe {
            ticker.scheduler.increment();
            ticker.scheduler.increment();
        }

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Ready(()));

        let queue = &mut ticker.scheduler.queue;

        let mut future = DelayFuture::new(timer.sender.clone(), counter.clone(), 20);

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);

        let mut future = DelayFuture::new(timer.sender.clone(), counter.clone(), 15);

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);

        if let Some(ticket) = queue.pop() {
            assert_eq!(ticket.expires, 1);
            ticket.waker.wake();
            assert_eq!(mockwaker.woke.load(Ordering::Relaxed), true)
        }

        if let Some(ticket) = queue.pop() {
            assert_eq!(ticket.expires, 15);
        }

        if let Some(ticket) = queue.pop() {
            assert_eq!(ticket.expires, 20);
        }
    }

    #[test]
    fn timer() {
        let (mut ticker, timer) = Box::leak(Box::new(Scheduler::<10>::new(1000.Hz()))).split();

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

        let mut executor = Box::leak(Box::new(Executor::new([task]))).get_handle();

        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), None);

        unsafe {
            ticker.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);

        ticker.scheduler.wake_tasks();
        executor.poll_tasks();

        assert_eq!(queue.pop(), None);

        unsafe {
            ticker.tick();
        }
        unsafe {
            ticker.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), None);

        unsafe {
            ticker.tick();
        }
        unsafe {
            ticker.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(4));
        assert_eq!(queue.pop(), None);

        unsafe {
            ticker.tick();
        }
        unsafe {
            ticker.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(5));
        assert_eq!(queue.pop(), None);

        for _ in 0..10 {
            unsafe {
                ticker.tick();
            }
            executor.poll_tasks();

            assert_eq!(queue.pop(), None);
        }

        unsafe {
            ticker.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(6));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn timeout() {
        let (ticker, timer) = Box::leak(Box::new(Scheduler::<10>::new(1000.Hz()))).split();

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
                ticker.scheduler.increment();
            }
        }

        ticker.scheduler.wake_tasks();

        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);

        for _ in 0..1000 {
            unsafe {
                ticker.scheduler.increment();
            }
            ticker.scheduler.wake_tasks();

            executor.poll_tasks();
            assert_eq!(queue.pop(), None);
        }

        unsafe {
            ticker.scheduler.increment();
        }
        ticker.scheduler.wake_tasks();

        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), None);
    }
}
