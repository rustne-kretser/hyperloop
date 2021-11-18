use heapless::binary_heap::{Kind, PeekMut};

use {
    alloc::{fmt, sync::Arc},
    crossbeam_queue::ArrayQueue,
    heapless::binary_heap::{BinaryHeap},
};

pub trait Item: Ord + Clone {}

struct Channel<T, K, const N: usize>
where T: Item,
      K: Kind {
    in_queue: Arc<ArrayQueue<T>>,
    out_queue: BinaryHeap<T, K, N>,
}

impl<T, K, const N: usize> Channel<T, K, N>
where T: Item,
      K: Kind {
    fn new() -> Self {
        Self {
            in_queue: Arc::new(ArrayQueue::new(N)),
            out_queue: BinaryHeap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Sender<T> {
    queue: Arc<ArrayQueue<T>>
}

impl<T> Sender<T>
where T: Item {
    fn new(queue: Arc<ArrayQueue<T>>) -> Self {
        Self {
            queue
        }
    }

    pub fn send(&self, item: T) -> Result<(), T> {
        if let Err(item) = self.queue.push(item) {
            Err(item)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct RecvError {}

impl fmt::Display for RecvError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "failed to receive item!")
    }
}

pub struct Receiver<T, K, const N: usize>
where T: Item,
      K: Kind {
    channel: Channel<T, K, N>,
}

impl<T, K, const N: usize> Receiver<T, K, N>
where T: Item,
      K: Kind {
    fn new(channel: Channel<T, K, N>) -> Self {
        Self {
            channel
        }
    }

    fn sort_incoming(&mut self) {
        while self.channel.out_queue.len() < N {
            if let Some(ticket) = self.channel.in_queue.pop() {
                if let Err(_) = self.channel.out_queue.push(ticket) {

                }
            } else {
                break;
            }
        }
    }

    pub fn recv(&mut self) -> Result<T, RecvError> {
        self.sort_incoming();

        if let Some(item) = self.channel.out_queue.pop() {
            Ok(item)
        } else {
            Err(RecvError {})
        }
    }

    pub fn peek_mut(&mut self) -> Result<PeekMut<T, K, N>, RecvError> {
        self.sort_incoming();

        if let Some(item) = self.channel.out_queue.peek_mut() {
            Ok(item)
        } else {
            Err(RecvError {})
        }
    }
}

pub fn channel<T, K, const N: usize>() -> (Sender<T>, Receiver<T, K, N>)
where T: Item,
      K: Kind {
    let pri_queue = Channel::new();
    (Sender::new(pri_queue.in_queue.clone()),
     Receiver::new(pri_queue))
}


#[cfg(test)]
mod tests {
    use heapless::binary_heap::Max;

    use super::Item;

    impl Item for u32 {}

    #[test]
    fn test_priority_channel() {
        use crate::priority_channel::*;

        let (sender, mut receiver) = channel::<u32, Max, 5>();

        sender.send(2).unwrap();
        sender.send(4).unwrap();
        sender.send(1).unwrap();
        sender.send(3).unwrap();
        sender.send(5).unwrap();

        assert_eq!(sender.send(6), Err(6));

        assert_eq!(receiver.recv(), Ok(5));
        assert_eq!(receiver.recv(), Ok(4));
        assert_eq!(receiver.recv(), Ok(3));
        assert_eq!(receiver.recv(), Ok(2));
        assert_eq!(receiver.recv(), Ok(1));
        assert_eq!(receiver.recv(), Err(RecvError {} ));
    }
}
