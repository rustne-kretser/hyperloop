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
use futures::{task::AtomicWaker, Stream, StreamExt};
use log::error;

use crate::priority_queue::{Min, PeekMut, PriorityQueue, PrioritySender};

type Tick = u64;

pub struct TickCounter {
    count: Tick,
    waker: AtomicWaker,
}

impl TickCounter {
    pub const fn new() -> Self {
        Self {
            count: 0,
            waker: AtomicWaker::new(),
        }
    }

    /// Increment tick count
    ///
    /// # Safety
    ///
    /// Updating the tick value is not atomic on 32-bit systems, so it
    /// would be possible to get an invalid reading if reading during
    /// a write. For this reason, this function should only be called
    /// from a high priority interrupt handler.
    pub unsafe fn increment(&mut self) {
        self.count += 1;
    }

    pub fn wake(&self) {
        self.waker.wake();
    }

    pub unsafe fn tick(&mut self) {
        self.increment();
        self.wake();
    }

    pub fn get_token(&self) -> TickCounterToken {
        TickCounterToken {
            counter: unsafe { &*(self as *const Self) },
        }
    }
}

#[derive(Clone)]
pub struct TickCounterToken {
    counter: &'static TickCounter,
}

impl TickCounterToken {
    pub fn register_waker(&self, waker: &Waker) {
        self.counter.waker.register(waker);
    }

    pub fn get_count(&self) -> Tick {
        self.counter.count
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
    counter: TickCounterToken,
    expires: Tick,
    started: bool,
}

impl DelayFuture {
    fn new(sender: PrioritySender<Ticket>, counter: TickCounterToken, expires: Tick) -> Self {
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
    fn new(
        future: F,
        sender: PrioritySender<Ticket>,
        counter: TickCounterToken,
        expires: Tick,
    ) -> Self {
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
    counter: TickCounterToken,
    sender: PrioritySender<Ticket>,
}

impl Timer {
    pub fn new(rate: Hertz, counter: TickCounterToken, sender: PrioritySender<Ticket>) -> Self {
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

struct TimerFuture {
    counter: TickCounterToken,
    expires: Option<Tick>,
}

impl TimerFuture {
    fn new(counter: TickCounterToken) -> Self {
        Self {
            counter,
            expires: None,
        }
    }
}

impl Stream for TimerFuture {
    type Item = ();

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(expires) = self.expires {
            if self.counter.get_count() >= expires {
                self.expires = None;
                return Poll::Ready(Some(()));
            }
        } else {
            self.expires = Some(self.counter.get_count() + 1_u64);
        }

        self.counter.register_waker(cx.waker());
        Poll::Pending
    }
}

pub struct Scheduler<const N: usize> {
    rate: Hertz,
    counter: TickCounterToken,
    queue: PriorityQueue<Ticket, Min, N>,
}

impl<const N: usize> Scheduler<N> {
    pub fn new(rate: Hertz, counter: TickCounterToken) -> Self {
        Self {
            rate,
            counter,
            queue: PriorityQueue::new(),
        }
    }

    pub unsafe fn get_timer(&self) -> Timer {
        Timer::new(self.rate, self.counter.clone(), self.queue.get_sender())
    }

    fn next_waker(&mut self) -> Option<Waker> {
        if let Some(ticket) = self.queue.peek_mut().as_mut() {
            if self.counter.get_count() > ticket.expires {
                return Some(PeekMut::pop(ticket).waker);
            }
        }

        None
    }

    pub async fn task(&mut self) {
        let mut timer = TimerFuture::new(self.counter.clone());

        loop {
            if let Some(waker) = self.next_waker() {
                waker.wake();
            } else {
                timer.next().await.unwrap()
            }
        }
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
        let counter = Box::leak(Box::new(TickCounter::new()));
        let token = counter.get_token();

        assert_eq!(token.get_count(), 0);

        unsafe { counter.increment() };
        assert_eq!(token.get_count(), 1);

        let mockwaker = Arc::new(MockWaker::new());
        let waker: Waker = mockwaker.clone().into();

        token.register_waker(&waker);
        counter.wake();

        assert_eq!(mockwaker.woke.load(Ordering::Relaxed), true);

        mockwaker.woke.store(false, Ordering::Relaxed);
        token.register_waker(&waker);

        unsafe {
            counter.tick();
        }
        assert_eq!(token.get_count(), 2);
        assert_eq!(mockwaker.woke.load(Ordering::Relaxed), true);
    }

    #[test]
    fn delay() {
        let counter = Box::leak(Box::new(TickCounter::new()));
        let token = counter.get_token();
        let scheduler: &'static mut Scheduler<10> =
            Box::leak(Box::new(Scheduler::new(1000.Hz(), token.clone())));
        let sender = unsafe { scheduler.queue.get_sender() };
        let mockwaker = Arc::new(MockWaker::new());
        let waker: Waker = mockwaker.clone().into();
        let mut cx = Context::from_waker(&waker);

        let mut future = DelayFuture::new(sender.clone(), token.clone(), 1);

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);
        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);
        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);
        assert_eq!(future.started, true);

        unsafe {
            counter.tick();
            counter.tick();
        }

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Ready(()));

        let mut future = DelayFuture::new(sender.clone(), token.clone(), 20);

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);

        let mut future = DelayFuture::new(sender.clone(), token.clone(), 15);

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
        let counter = Box::leak(Box::new(TickCounter::new()));
        let token = counter.get_token();
        let scheduler: &'static mut Scheduler<10> =
            Box::leak(Box::new(Scheduler::new(1000.Hz(), token.clone())));
        let timer = unsafe { scheduler.get_timer() };
        let mut executor = Box::leak(Box::new(Executor::<10>::new())).get_ref();
        let queue = Arc::new(ArrayQueue::new(10));

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

        let task1 = Task::new(move || scheduler.task(), 1);
        let task2 = Task::new(test_future(queue.clone(), timer.clone()), 1);

        task1.add_to_executor(executor.get_sender()).unwrap();
        task2.add_to_executor(executor.get_sender()).unwrap();

        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), None);

        unsafe {
            counter.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);

        counter.wake();
        executor.poll_tasks();

        assert_eq!(queue.pop(), None);

        unsafe {
            counter.tick();
        }
        unsafe {
            counter.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), None);

        unsafe {
            counter.tick();
        }
        unsafe {
            counter.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(4));
        assert_eq!(queue.pop(), None);

        unsafe {
            counter.tick();
        }
        unsafe {
            counter.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(5));
        assert_eq!(queue.pop(), None);

        for _ in 0..10 {
            unsafe {
                counter.tick();
            }
            executor.poll_tasks();

            assert_eq!(queue.pop(), None);
        }

        unsafe {
            counter.tick();
        }
        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(6));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn timeout() {
        let counter = Box::leak(Box::new(TickCounter::new()));
        let token = counter.get_token();
        let scheduler: &'static mut Scheduler<10> =
            Box::leak(Box::new(Scheduler::new(1000.Hz(), token.clone())));
        let timer = unsafe { scheduler.get_timer() };
        let mut executor = Box::leak(Box::new(Executor::<10>::new())).get_ref();
        let queue = Arc::new(ArrayQueue::new(10));

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

        let task1 = Task::new(move || scheduler.task(), 1);
        let task2 = Task::new(waiting_future(queue.clone(), timer), 1);

        task1.add_to_executor(executor.get_sender()).unwrap();
        task2.add_to_executor(executor.get_sender()).unwrap();

        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), None);

        for _ in 0..101 {
            unsafe {
                counter.increment();
            }
        }

        counter.wake();

        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);

        for _ in 0..1000 {
            unsafe {
                counter.increment();
            }
            counter.wake();

            executor.poll_tasks();

            assert_eq!(queue.pop(), None);
        }

        unsafe {
            counter.increment();
        }
        counter.wake();

        executor.poll_tasks();

        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), None);
    }
}
