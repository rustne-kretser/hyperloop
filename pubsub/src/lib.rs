#![no_std]

extern crate alloc;

pub mod mpsc {
    use {
        futures::{
            stream::{Stream, StreamExt, Next},
            task::AtomicWaker,
        },
        crossbeam_queue::{ArrayQueue},
        alloc::sync::Arc,
        core::pin::Pin,
        core::task::{Context, Poll},
    };

    pub struct Channel<T> {
        queue: ArrayQueue<T>,
        waker: AtomicWaker,
    }

    impl<T> Channel<T> {
        pub fn new(capacity: usize) -> Channel<T> {
            Channel {
                queue: ArrayQueue::new(capacity),
                waker: AtomicWaker::new(),
            }
        }

        pub fn channel(self: Arc<Self>) -> (Sender<T>, Receiver<T>) {
            (Sender { channel: self.clone() },
             Receiver { channel: self.clone() })
        }
    }

    #[derive(Clone)]
    pub struct Sender<T> {
        channel: Arc<Channel<T>>,
    }

    impl<T> Sender<T> {
        pub fn try_send(&mut self, msg: T) -> Result<(), T> {
            match self.channel.queue.push(msg) {
                Ok(value) => {
                    self.channel.waker.wake();
                    Ok(value)
                },
                Err(msg) => Err(msg)
            }
        }
    }

    pub struct Receiver<T> {
        channel: Arc<Channel<T>>,
    }

    impl<T> Receiver<T> {
        pub fn recv(&mut self) -> Next<Self> {
            self.next()
        }
    }

    impl<T> Stream for Receiver<T> {
        type Item = T;

        fn poll_next(self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<T>> {
            if let Some(item) = self.channel.queue.pop() {
                return Poll::Ready(Some(item));
            }

            self.channel.waker.register(&cx.waker());

            match self.channel.queue.pop() {
                Some(item) => {
                    self.channel.waker.take();
                    Poll::Ready(Some(item))
                },
                None => Poll::Pending,
            }
        }
    }

    pub fn channel<T>(capacity: usize) -> (Sender<T>, Receiver<T>) {
        let channel = Arc::new(Channel::new(capacity));
        channel.channel()
    }
}

pub mod pubsub {
    use {
        crate::mpsc::{Sender, Receiver, channel},
        alloc::{
            vec::Vec,
            collections::{
                BTreeMap,
            },
        },
        log::error,
    };

    pub trait ClassMember<C> {
        fn get_class(&self) -> C;
    }

    pub struct Subscriber<T: Copy> {
        sender: Sender<T>,
        pub receiver: Receiver<T>,
    }

    impl<T: Copy> Subscriber<T> {
        pub fn new(capacity: usize) -> Subscriber<T> {
            let (sender, receiver) = channel(capacity);
            Subscriber { sender, receiver }
        }
    }

    pub struct PubSub<T: Copy + ClassMember<C>, C: Ord> {
        incoming: (Sender<T>, Receiver<T>),
        subscribers: BTreeMap<C, Vec<Sender<T>>>,
    }

    impl<T: Copy + ClassMember<C>, C: Ord> PubSub<T, C> {
        pub fn new(capacity: usize) -> PubSub<T, C> {
            PubSub {
                incoming: channel(capacity),
                subscribers: BTreeMap::new()
            }
        }

        pub fn subscribe(&mut self, class: C, subscriber: &Subscriber<T>) {
            let senders = self.subscribers.entry(class)
                .or_insert_with(|| Vec::new());

            senders.push(subscriber.sender.clone());
        }

        async fn publish(&mut self, message: T) {
            let class = message.get_class();
            if let Some(subscribers) = self.subscribers.get_mut(&class) {
                for subscriber in subscribers {

                    if let Err(_err) = subscriber.try_send(message) {
                        error!("Queue full!");
                    }
                }
            }
        }

        pub fn get_sender(&self) -> Sender<T> {
            self.incoming.0.clone()
        }

        pub async fn task(mut self) {
            while let Some(message) = self.incoming.1.recv().await {
                self.publish(message).await;
            }
        }
    }
}


mod test_def {
    use crate::pubsub::{
        ClassMember,
    };

    #[allow(dead_code)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub enum Class {
        Class1,
        Class2,
        Class3,
    }

    #[allow(dead_code)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub enum Message {
        SomeEvent(u8),
        OtherEvent(u32),
        ThirdEvent(u8),
    }

    use Message::*;

    impl ClassMember<Class> for Message {
        fn get_class(&self) -> Class {
            match self {
                SomeEvent{..} | OtherEvent{..} => Class::Class1,
                ThirdEvent{..} => Class::Class2,
            }
        }
    }
}

#[cfg(test)]
#[macro_use]
extern crate std;
mod tests {
    #[test]
    fn test_main() {
        use hyperloop::hyperloop::*;
        use crate::test_def::*;
        use crate::pubsub::*;
        use {
            crate::mpsc::{Sender, Receiver},
        };
        use alloc::boxed::Box;

        let mut hp = Hyperloop::new(10);
        let mut pubsub: PubSub<Message, Class> = PubSub::new(10);

        async fn baba(mut sender: Sender<Message>, mut receiver: Receiver<Message>) {
            loop {
                if let Some(event) = receiver.recv().await {
                    match event {
                        Message::SomeEvent(value) => {
                            sender.try_send(Message::ThirdEvent(42)).unwrap();
                            println!("Some event: {}", value)
                        },
                        Message::OtherEvent(value) => {println!("Other event: {}", value)},
                        Message::ThirdEvent(value) => {println!("Third event: {}", value)},
                    }
                };
            }
        }

        async fn ganoush(mut sender: Sender<Message>, mut receiver: Receiver<Message>) {
            if let Err(_err) = sender.try_send(Message::SomeEvent(12)) {
                println!("Failed to send message!");
            }

            loop {
                if let Some(event) = receiver.recv().await {
                    match event {
                        Message::SomeEvent(value) => {
                            println!("Some event: {}", value)
                        },
                        Message::OtherEvent(value) => {println!("Other event: {}", value)},
                        Message::ThirdEvent(value) => {println!("Third event: {}", value)},
                    }
                };
            }
        }

        {
            let subscriber: Subscriber<Message> = Subscriber::new(10);
            pubsub.subscribe(Class::Class2, &subscriber);

            let ganoush = hp.add_task(Box::pin(ganoush(pubsub.get_sender(), subscriber.receiver)), 1);
            hp.schedule_task(ganoush);
        }

        {
            let subscriber: Subscriber<Message> = Subscriber::new(10);
            pubsub.subscribe(Class::Class1, &subscriber);

            let baba = hp.add_task(Box::pin(baba(pubsub.get_sender(), subscriber.receiver)), 2);
            hp.schedule_task(baba);
        }

        let mut sender = pubsub.get_sender();
        let ps_task = hp.add_task(Box::pin(pubsub.task()), 3);
        hp.schedule_task(ps_task);

        sender.try_send(Message::SomeEvent(8)).unwrap();
        sender.try_send(Message::OtherEvent(8)).unwrap();
        sender.try_send(Message::ThirdEvent(8)).unwrap();

        hp.poll_tasks();
    }
}
