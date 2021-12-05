use core::{fmt, marker::PhantomData, ops::Add, pin::Pin, sync::atomic::Ordering, task::{Context, Poll, Waker}};

use atomic_traits::{Atomic, NumOps};
use embedded_time::{duration::{Duration, Milliseconds}, fixed_point::FixedPoint, rate::{Hertz, Rate}};

use core::future::Future;
use futures::{Stream, StreamExt, task::AtomicWaker};
use heapless::binary_heap::{Min, PeekMut};
use log::error;

use crate::priority_channel::{Item, Receiver, Sender, channel};

pub trait Tick: 'static + Add<Output = Self> + Sized + From<u32> + Sync + Send + Copy + Ord + fmt::Display + Unpin {}

impl Tick for u32 {}

impl Tick for u64 {}

pub trait TimerState {
    type Tick;

    fn set_count(&self, value: Self::Tick);

    fn add_count(&self, value: Self::Tick);

    fn increment_count(&self);

    fn wake(&self);

    fn tick(&self) {
        self.increment_count();
        self.wake();
    }
}

pub trait TimerStateRef: Clone + Unpin {
    type Tick;

    fn get_count(&self) -> Self::Tick;

    fn register_waker(&self, waker: &Waker);
}

pub struct AtomicTimerState<T, A>
where T: Tick,
      A: Atomic<Type = T> + NumOps {
    atomic_waker: AtomicWaker,
    counter: A,
    phantom: PhantomData<T>,
}

impl<T, A> AtomicTimerState<T, A>
where T: Tick ,
      A: Atomic<Type = T> + NumOps {
    pub fn new() -> Self {
        Self {
            atomic_waker: AtomicWaker::new(),
            counter: A::new(0_u32.into()),
            phantom: PhantomData,
        }
    }

    pub fn get_ref(&'static self) -> AtomicTimerStateRef<T, A> {
        AtomicTimerStateRef::new(&self)
    }
}

impl<T, A> TimerState for AtomicTimerState<T, A>
where T: Tick,
      A: 'static + Atomic<Type = T> + NumOps {
    type Tick = T;

    fn set_count(&self, value: Self::Tick) {
        self.counter.store(value, Ordering::Relaxed);
    }

    fn add_count(&self, value: Self::Tick) {
        self.counter.fetch_add(value, Ordering::Relaxed);
    }

    fn increment_count(&self) {
        self.add_count(1_u32.into());
    }

    fn wake(&self) {
        self.atomic_waker.wake();
    }
}

pub struct AtomicTimerStateRef<T, A>
where T: Tick,
      A: 'static + Atomic<Type = T> + NumOps {
    state: &'static AtomicTimerState<T, A>,
}

impl<T, A> AtomicTimerStateRef<T, A>
where T: Tick,
      A: 'static + Atomic<Type = T> + NumOps {
    fn new(state: &'static AtomicTimerState<T, A>) -> Self {
        Self {
            state,
        }
    }
}

impl<T, A> Unpin for AtomicTimerStateRef<T, A>
where T: Tick,
      A: 'static + Atomic<Type = T> + NumOps {}

impl<T, A> Clone for AtomicTimerStateRef<T, A>
where T: Tick,
      A: 'static + Atomic<Type = T> + NumOps {
    fn clone(&self) -> Self {
        Self { state: self.state.clone() }
    }
}

impl<T, A> TimerStateRef for AtomicTimerStateRef<T, A>
where T: Tick,
      A: Atomic<Type = T> + NumOps {
    type Tick = T;

    fn get_count(&self) -> Self::Tick {
        self.state.counter.load(Ordering::Relaxed)
    }

    fn register_waker(&self, waker: &Waker) {
        self.state.atomic_waker.register(waker);
    }
}

#[derive(Debug, Clone)]
pub struct Ticket<T>
where T: Tick {
    expires: T,
    waker: Waker,
}

impl<T> Ticket<T>
where T: Tick {
    fn new(expires: T, waker: Waker) -> Self {
        Self {
            expires,
            waker,
        }
    }
}

impl<T> Item for Ticket<T>
where T: Tick {}

impl<T> PartialEq for Ticket<T>
where T: Tick {
    fn eq(&self, other: &Self) -> bool {
        self.expires == other.expires
    }
}

impl<T> Eq for Ticket<T>
where T: Tick {}

impl<T> PartialOrd for Ticket<T>
where T: Tick {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Ticket<T>
where T: Tick {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.expires.cmp(&other.expires)
    }
}

struct DelayFuture<S, T>
where T: Tick,
      S: TimerStateRef<Tick = T> {
    sender: Sender<Ticket<T>>,
    state: S,
    expires: T,
    started: bool,
}

impl<S, T> DelayFuture<S, T>
where T: Tick,
      S: TimerStateRef<Tick = T> {
    fn new(sender: Sender<Ticket<T>>, state: S, expires: T) -> Self {
        Self {
            sender,
            state,
            expires,
            started: false,
        }
    }
}

impl<S, T> Unpin for DelayFuture<S, T>
where T: Tick,
      S: TimerStateRef<Tick = T>  {}

impl<S, T> Future for DelayFuture<S, T>
where T: Tick,
      S: TimerStateRef<Tick = T> {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if !self.started {
            if let Err(_) = self.sender.send(Ticket::new(self.expires, cx.waker().clone())) {
                error!("failed to send ticket");
            }
            self.started = true;
            Poll::Pending
        } else {
            // Delay until tick count is greater than the
            // expiration. This ensures that we wait for no less than
            // the specified duration, and possibly one tick longer
            // than desired.
            if self.state.get_count() > self.expires {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }
    }
}

pub struct TimeoutFuture<S, T, F>
where T: Tick,
      S: TimerStateRef<Tick = T>,
      F: Future {
    future: F,
    delay: DelayFuture<S, T>,
}

impl<S, T, F> TimeoutFuture<S, T, F>
where T: Tick,
      S: TimerStateRef<Tick = T>,
      F: Future {
    fn new(future: F, sender: Sender<Ticket<T>>, state: S, expires: T) -> Self {
        Self {
            future,
            delay: DelayFuture::new(sender, state, expires),
        }
    }
}

impl<S, T, F> Future for TimeoutFuture<S, T, F>
where T: Tick,
      S: TimerStateRef<Tick = T>,
      F: Future {
    type Output = Result<F::Output, ()>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let (future, delay) = unsafe {
            let this = self.get_unchecked_mut();
            (Pin::new_unchecked(&mut this.future),
             Pin::new_unchecked(&mut this.delay))
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
pub struct Timer<S, T>
where T: Tick,
      S: TimerStateRef<Tick = T> {
    rate: Hertz,
    state: S,
    sender: Sender<Ticket<T>>,
}

impl<S, T> Timer<S, T>
where S: TimerStateRef<Tick = T>,
      T: Tick {
    pub fn new(rate: Hertz, state: S, sender: Sender<Ticket<T>>) -> Self {
        Self {
            rate,
            state,
            sender,
        }
    }

    fn get_rate(&self) -> Hertz {
        self.rate
    }

    fn get_count(&self) -> T {
        self.state.get_count()
    }

    fn delay_to_ticks<D: Duration + Into<Milliseconds>>(&self, duration: D) -> T {
        let ms: Milliseconds = duration.into();
        let rate = self.get_rate().to_duration::<Milliseconds>().unwrap();

        assert!(ms.integer() == 0 || ms >= rate);

        let ticks = ms.integer() / rate.integer();

        self.get_count() + ticks.into()
    }

    pub fn delay(&self, duration: Milliseconds) -> impl Future {
        DelayFuture::new(self.sender.clone(),
                         self.state.clone(),
                         self.delay_to_ticks(duration))
    }

    pub fn timeout<F: Future>(&self, future: F, duration: Milliseconds)
                   -> TimeoutFuture<S, T, F> {
        TimeoutFuture::new(future,
                           self.sender.clone(),
                           self.state.clone(),
                           self.delay_to_ticks(duration))
    }
}

struct TimerFuture<T, S>
where T: Tick,
      S: TimerStateRef<Tick = T> {
    state: S,
    expires: Option<T>,
}

impl<T, S> TimerFuture<T, S>
where T: Tick,
      S: TimerStateRef<Tick = T> {
    fn new(state: S) -> Self {
        Self {
            state,
            expires: None,
        }
    }
}

impl<T, S> Stream for TimerFuture<T, S>
where T: Tick,
      S: TimerStateRef<Tick = T> {
    type Item = ();

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(expires) = self.expires {
            if self.state.get_count() >= expires {
                self.expires = None;
                return Poll::Ready(Some(()))
            }
        } else {
            self.expires = Some(self.state.get_count() + 1.into());
        }

        self.state.register_waker(cx.waker());
        Poll::Pending
    }
}

pub struct Scheduler<S, T, const N: usize>
where T: Tick,
      S: TimerStateRef<Tick = T>  {
    rate: Hertz,
    state: S,
    sender: Sender<Ticket<T>>,
    receiver: Receiver<Ticket<T>, Min, N>
}

impl<S, T, const N: usize> Scheduler<S, T, N>
where T: Tick,
      S: TimerStateRef<Tick = T>  {
    pub fn new(rate: Hertz, state: S) -> Self {
        let (sender, receiver) = channel();

        Self {
            rate,
            state,
            sender,
            receiver,
        }
    }

    pub fn get_timer(&self) -> Timer<S, T> {
        Timer::new(self.rate, self.state.clone(), self.sender.clone())
    }

    fn next_waker(&mut self) -> Option<Waker> {
        if let Ok(ticket) = self.receiver.peek_mut() {
            if self.state.get_count() > ticket.expires {
                return Some(PeekMut::pop(ticket).waker);
            }
        }

        None
    }

    pub async fn task(&mut self) {
        let mut timer = TimerFuture::new(self.state.clone());

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
    use core::sync::atomic::AtomicU32;

    use crate::executor::Executor;
    use crate::common::tests::MockWaker;

    use super::*;

    use alloc::{boxed::Box, sync::Arc};
    use crossbeam_queue::ArrayQueue;
    use embedded_time::rate::Extensions;
    use embedded_time::duration::Extensions as Ext;
    use core::future::Future;

    use log::{Record, Level, Metadata};

    struct SimpleLogger;

    use log::LevelFilter;

    static LOGGER: SimpleLogger = SimpleLogger;

    fn log_init() {
        let _ = log::set_logger(&LOGGER)
            .map(|()| log::set_max_level(LevelFilter::Info));
    }


    impl log::Log for SimpleLogger {
        fn enabled(&self, metadata: &Metadata) -> bool {
            metadata.level() <= Level::Info
        }

        fn log(&self, record: &Record) {
            if self.enabled(record.metadata()) {
                println!("{} - {}", record.level(), record.args());
            }
        }

        fn flush(&self) {}
    }

    #[test]
    fn state() {
        let state: &'static AtomicTimerState<u32, AtomicU32> = Box::leak(
            Box::new(AtomicTimerState::new()));
        let stateref = state.get_ref();

        assert_eq!(stateref.get_count(), 0);

        state.increment_count();
        assert_eq!(stateref.get_count(), 1);

        state.add_count(10);
        assert_eq!(stateref.get_count(), 11);

        state.set_count(0);
        assert_eq!(stateref.get_count(), 0);

        let mockwaker = Arc::new(MockWaker::new());
        let waker: Waker = mockwaker.clone().into();

        stateref.register_waker(&waker);
        state.wake();

        assert_eq!(mockwaker.woke.load(Ordering::Relaxed), true);

        mockwaker.woke.store(false, Ordering::Relaxed);
        stateref.register_waker(&waker);

        state.tick();
        assert_eq!(stateref.get_count(), 1);
        assert_eq!(mockwaker.woke.load(Ordering::Relaxed), true);
    }

    #[test]
    fn delay() {
        let state: &'static AtomicTimerState<u32, AtomicU32> = Box::leak(
            Box::new(AtomicTimerState::new()));
        let stateref = state.get_ref();
        let (sender, mut receiver) = channel::<Ticket<u32>, Min, 10>();
        let mockwaker = Arc::new(MockWaker::new());
        let waker: Waker = mockwaker.clone().into();
        let mut cx = Context::from_waker(&waker);

        let mut future = DelayFuture::new(sender.clone(), stateref.clone(), 10);

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);
        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);
        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);
        assert_eq!(future.started, true);

        state.set_count(11);

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Ready(()));

        let mut future = DelayFuture::new(sender.clone(), stateref.clone(), 20);

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);

        let mut future = DelayFuture::new(sender.clone(), stateref.clone(), 15);

        assert_eq!(Pin::new(&mut future).poll(&mut cx), Poll::Pending);


        if let Ok(ticket) = receiver.recv() {
            assert_eq!(ticket.expires, 10);
            ticket.waker.wake();
            assert_eq!(mockwaker.woke.load(Ordering::Relaxed), true)
        }

        if let Ok(ticket) = receiver.recv() {
            assert_eq!(ticket.expires, 15);
        }

        if let Ok(ticket) = receiver.recv() {
            assert_eq!(ticket.expires, 20);
        }
    }

    #[test]
    fn timer() {
        let state: &'static AtomicTimerState<u32, AtomicU32> = Box::leak(
            Box::new(AtomicTimerState::new()));
        let stateref = state.get_ref();
        let scheduler: &'static mut Scheduler<_, _, 10>
            = Box::leak(Box::new(Scheduler::new(1000.Hz(), stateref.clone())));
        let timer = scheduler.get_timer();
        let mut executor = Box::new(Executor::<10>::new());
        let queue =  Arc::new(ArrayQueue::new(10));

        log_init();

        async fn test_future(queue: Arc<ArrayQueue<u32>>,
                             timer: Timer<AtomicTimerStateRef<u32, AtomicU32>, u32>) {
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

        executor.add_task(Box::pin(scheduler.task()), 1).unwrap();
        executor.add_task(Box::pin(test_future(queue.clone(), timer.clone())), 1).unwrap();

        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), None);

        state.tick();
        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);

        state.wake();
        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), None);

        state.tick();
        state.tick();
        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), None);

        state.tick();
        state.tick();
        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), Some(4));
        assert_eq!(queue.pop(), None);

        state.tick();
        state.tick();
        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), Some(5));
        assert_eq!(queue.pop(), None);

        for _ in 0..10 {
            state.tick();
            unsafe { executor.poll_tasks(); }

            assert_eq!(queue.pop(), None);
        }

        state.tick();
        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), Some(6));
        assert_eq!(queue.pop(), None);
    }

    #[test]
    fn timeout() {
        let state: &'static AtomicTimerState<u32, AtomicU32> = Box::leak(
            Box::new(AtomicTimerState::new()));
        let stateref = state.get_ref();
        let scheduler: &'static mut Scheduler<_, _, 10>
            = Box::leak(Box::new(Scheduler::new(1000.Hz(), stateref.clone())));
        let timer = scheduler.get_timer();
        let mut executor = Executor::<10>::new();
        let queue =  Arc::new(ArrayQueue::new(10));

        log_init();

        async fn slow_future(timer: Timer<AtomicTimerStateRef<u32, AtomicU32>, u32>) {
            timer.delay(1000.milliseconds()).await;
        }

        async fn waiting_future(queue: Arc<ArrayQueue<u32>>,
                                timer: Timer<AtomicTimerStateRef<u32, AtomicU32>, u32>) {
            queue.push(1).unwrap();

            assert_eq!(timer.timeout(slow_future(timer.clone()), 100.milliseconds()).await,
                       Err(()));
            queue.push(2).unwrap();

            assert_eq!(timer.timeout(slow_future(timer.clone()), 1001.milliseconds()).await,
                       Ok(()));
            queue.push(3).unwrap();
        }

        executor.add_task(Box::pin(scheduler.task()), 1).unwrap();
        executor.add_task(Box::pin(waiting_future(queue.clone(), timer)), 1).unwrap();

        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), None);

        state.add_count(101);
        state.wake();

        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), Some(2));
        assert_eq!(queue.pop(), None);

        for _ in 0..1000 {
            state.increment_count();
            state.wake();

            unsafe { executor.poll_tasks(); }

            assert_eq!(queue.pop(), None);
        }

        state.increment_count();
        state.wake();

        unsafe { executor.poll_tasks(); }

        assert_eq!(queue.pop(), Some(3));
        assert_eq!(queue.pop(), None);
    }
}