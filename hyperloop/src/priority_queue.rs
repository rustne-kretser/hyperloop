use core::{marker::PhantomData, ops::Deref, sync::atomic::{AtomicUsize, Ordering}};

pub enum HeapKind {
    Max,
    Min,
}

pub struct Min {}
pub struct Max {}

pub trait Kind {
    fn kind() -> HeapKind;
}

impl Kind for Min {
    fn kind() -> HeapKind {
        HeapKind::Min
    }
}

impl Kind for Max {
    fn kind() -> HeapKind {
        HeapKind::Max
    }
}

struct Node<'a, T, K, const N: usize>
where T: PartialOrd,
      K: Kind {
    heap: &'a PriorityQueue<T, K, N>,
    pos: usize,
    _kind: PhantomData<K>,
}

impl <'a, T, K, const N: usize> Node<'a, T, K, N>
where T: PartialOrd + 'static,
      K: Kind + 'static {
    fn new(heap: &'a PriorityQueue<T, K, N>, pos: usize) -> Self {
        Self {
            heap,
            pos,
            _kind: PhantomData,
        }
    }

    fn get_node(&self, index: usize) -> Option<Self>  {
        if index < self.heap.heap_size {
            Some(Node::new(self.heap, index))
        } else {
            None
        }
    }

    fn children(&self) -> (Option<Self>, Option<Self>) {
        let left = 2 * self.pos + 1;
        let right = left + 1;

        (self.get_node(left), self.get_node(right))
    }

    fn highest_priority_child(&self) -> Option<Self> {
        let (left, right) = self.children();

        if let Some(right) = right {
            let left = left.unwrap();

            if left.is_higher_priority(&right) {
                return Some(left);
            } else {
                return Some(right);
            }
        } else {
            if let Some(left) = left {
                return Some(left);
            }
        }

        None
    }

    fn parent(&self) -> Option<Self> {
        if self.pos > 0 {
            let index = (self.pos - 1) / 2;
            self.get_node(index)
        } else {
            None
        }
    }

    fn item(&self) -> &T {
        self.heap.slots[self.pos].as_ref().unwrap()
    }

    unsafe fn slot_mut(&self) -> &mut Option<T> {
        self.heap.slot_mut(self.pos)
    }

    fn swap(self, other: Self) -> Self {
        let slot = unsafe { self.slot_mut() };
        let other_slot = unsafe { other.slot_mut() };

        let item = slot.take();
        *slot = other_slot.take();
        *other_slot = item;

        other
    }

    fn is_higher_priority(&self, other: &Self) -> bool {
        match K::kind() {
            HeapKind::Max => self.item() > other.item(),
            HeapKind::Min => self.item() < other.item(),
        }
    }
}

pub trait Sender: Clone {
    type Item;

    fn send(&self, item: Self::Item) -> Result<(), Self::Item>;
}

pub struct PrioritySender<'a, T, K, const N: usize>
where T: PartialOrd,
      K: Kind {
    queue: &'a PriorityQueue<T, K, N>,
}

impl<'a, T, K, const N: usize> Clone for PrioritySender<'a, T, K, N>
where T: PartialOrd,
      K: Kind {
    fn clone(&self) -> Self {
        Self { queue: self.queue.clone() }
    }
}

impl<'a, T, K, const N: usize> Sender for PrioritySender<'a, T, K, N>
where T: PartialOrd + 'static,
      K: Kind + 'static {
    type Item = T;

    fn send(&self, item: T) -> Result<(), T> {
        self.queue.push(item)
    }
}

pub struct PeekMut<'a, T, K, const N: usize>
where T: PartialOrd,
      K: Kind {
    queue: &'a mut PriorityQueue<T, K, N>
}

impl<'a, T, K, const N: usize> PeekMut<'a, T, K, N>
where T: PartialOrd + 'static ,
      K: Kind + 'static  {
    pub fn pop(&mut self) -> T {
        self.queue.pop().unwrap()
    }
}

impl<'a, T, K, const N: usize> Deref for PeekMut<'a, T, K, N>
where T: PartialOrd,
      K: Kind {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.queue.slots[0].as_ref().unwrap()
    }
}


pub struct PriorityQueue<T, K, const N: usize>
where T: PartialOrd,
      K: Kind {
    slots: [Option<T>; N],
    available: AtomicUsize,
    stack_size: AtomicUsize,
    heap_size: usize,
    _phantom: PhantomData<K>,
}

impl<T, K, const N: usize> PriorityQueue<T, K, N>
where T: PartialOrd + 'static,
      K: Kind + 'static {
    pub fn new() -> Self {
        Self {
            slots: [(); N].map(|_| None),
            available: AtomicUsize::new(N),
            stack_size: AtomicUsize::new(0),
            heap_size: 0,
            _phantom: PhantomData,
        }
    }

    pub fn get_sender(&self) -> impl Sender<Item = T> {
        let queue = unsafe { &*(self as *const Self) };

        PrioritySender {
            queue
        }
    }

    unsafe fn slot_mut(&self, index: usize) -> &mut Option<T> {
        &mut *((&self.slots[index]
                as *const Option<T>)
               as *mut Option<T>)
    }

    fn stack_push(&self, item: T) -> Result<(), T> {
        loop {
            let stack_size = self.stack_size.load(Ordering::Relaxed);

            if stack_size < N - self.heap_size {
                if let Ok(_) = self.stack_size.compare_exchange(stack_size, stack_size + 1,
                                                                Ordering::Relaxed,
                                                                Ordering::Relaxed) {
                    let index = N - stack_size - 1;

                    unsafe {
                        let slot = self.slot_mut(index);
                        *slot = Some(item);
                    }

                    break Ok(());
                }
            } else {
                break Err(item);
            }
        }
    }

    pub fn push(&self, item: T) -> Result<(), T> {
        loop {
            let available = self.available.load(Ordering::Relaxed);

            if available > 0 {
                if let Ok(_) = self.available.compare_exchange(available, available - 1,
                                                               Ordering::Relaxed,
                                                               Ordering::Relaxed) {
                    break;
                }
            } else {
                return Err(item);
            }
        }

        self.stack_push(item)
    }

    fn get_node(&self, index: usize) -> Node<T, K, N> {
        Node::new(self, index)
    }

    fn get_root(&self) -> Node<T, K, N> {
        self.get_node(0)
    }

    fn get_last(&self) -> Node<T, K, N> {
        self.get_node(self.heap_size - 1)
    }

    fn pop_stack(&mut self) -> Option<T> {
        loop {
            let stack_size = self.stack_size.load(Ordering::Relaxed);

            if stack_size > 0 {
                let index = N - stack_size;
                let item = self.slots[index].take().unwrap();

                if let Ok(_) = self.stack_size.compare_exchange(stack_size, stack_size - 1,
                                                                Ordering::Relaxed,
                                                                Ordering::Relaxed) {
                    break Some(item);
                } else {
                    self.slots[index] = Some(item);
                }
            } else {
                break None;
            }
        }
    }

    fn move_to_heap(&mut self) -> Result<(), ()> {
        if let Some(item) = self.pop_stack() {
            let _ = self.heap_insert(item);
            Ok(())
        } else {
            Err(())
        }
    }

    fn sort(&mut self) {
        while self.move_to_heap().is_ok() {
        }
    }

    fn heap_insert(&mut self, item: T) -> Result<(), T> {
        let index = self.heap_size;

        if index < N {
            self.slots[index] = Some(item);

            self.heap_size += 1;

            let mut node = self.get_node(index);

            loop {
                if let Some(parent) = node.parent() {
                    if node.is_higher_priority(&parent) {
                        node = node.swap(parent);
                        continue;
                    }
                }

                break;
            }

            Ok(())
        } else {
            Err(item)
        }
    }

    fn take_root(&mut self) -> Option<T> {
        if let Some(item) = self.slots[0].take() {
            {
                let root = self.get_root();
                let last = self.get_last();
                last.swap(root);
            }
            self.heap_size -= 1;

            Some(item)
        } else {
            None
        }
    }

    fn heap_pop(&mut self) -> Option<T> {
        if let Some(item) = self.take_root() {
            let mut node = self.get_root();

            loop {
                if let Some(child) = node.highest_priority_child() {
                    if child.is_higher_priority(&node) {
                        node = node.swap(child);
                        continue;
                    }
                }

                break;
            }

            Some(item)
        } else {
            None
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        self.sort();

        if let Some(item) = self.heap_pop() {
            loop {
                let available = self.available.load(Ordering::Relaxed);

                if let Ok(_) = self.available.compare_exchange(available, available + 1,
                                                               Ordering::Relaxed,
                                                               Ordering::Relaxed) {
                    break Some(item);
                }
            }
        } else  {
            None
        }
    }

    pub fn peek_mut<'a>(&'a mut self) -> Option<PeekMut<'a, T, K, N>> {
        self.sort();

        if self.heap_size > 0 {
            Some(
                PeekMut {
                    queue: self,
                }
            )
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heap() {
        let mut heap: PriorityQueue<u32, Min, 10> = PriorityQueue::new();

        assert!(heap.heap_pop().is_none());

        heap.heap_insert(2).unwrap();
        heap.heap_insert(1).unwrap();
        heap.heap_insert(10).unwrap();
        heap.heap_insert(5).unwrap();
        heap.heap_insert(8).unwrap();
        heap.heap_insert(3).unwrap();
        heap.heap_insert(9).unwrap();
        heap.heap_insert(4).unwrap();
        heap.heap_insert(7).unwrap();
        heap.heap_insert(6).unwrap();

        assert_eq!(heap.heap_pop(), Some(1));
        assert_eq!(heap.heap_pop(), Some(2));
        assert_eq!(heap.heap_pop(), Some(3));
        assert_eq!(heap.heap_pop(), Some(4));
        assert_eq!(heap.heap_pop(), Some(5));
        assert_eq!(heap.heap_pop(), Some(6));
        assert_eq!(heap.heap_pop(), Some(7));
        assert_eq!(heap.heap_pop(), Some(8));
        assert_eq!(heap.heap_pop(), Some(9));
        assert_eq!(heap.heap_pop(), Some(10));
        assert!(heap.heap_pop().is_none());
    }

    #[test]
    fn stack() {
        let mut heap: PriorityQueue<u32, Min, 10> = PriorityQueue::new();

        for i in 0..10 {
            heap.stack_push(i).unwrap();
        }

        assert!(heap.stack_push(11).is_err());

        for i in (0..10).rev() {
            assert_eq!(heap.pop_stack(), Some(i));
        }

        assert!(heap.pop_stack().is_none());

        for i in 0..5 {
            heap.stack_push(i).unwrap();
        }

        for i in (0..5).rev() {
            assert_eq!(heap.pop_stack(), Some(i));
        }

        assert!(heap.pop_stack().is_none());
    }

    #[test]
    fn channel() {
        let mut queue: PriorityQueue<u32, Min, 10> = PriorityQueue::new();
        let sender = queue.get_sender();

        for i in 0..10 {
            sender.send(i).unwrap();
        }

        assert!(sender.send(10).is_err());

        queue.sort();

        for i in 0..10 {
            assert_eq!(queue.pop(), Some(i));
        }

        for i in 0..5 {
            sender.send(i).unwrap();
        }

        queue.sort();

        for i in 0..5 {
            sender.send(i).unwrap();
        }

        for i in 0..5 {
            assert_eq!(queue.pop(), Some(i));
            assert_eq!(queue.pop(), Some(i));
        }

        sender.send(42).unwrap();

        let item = queue.peek_mut();
        assert!(item.is_some());
        assert_eq!(*item.as_ref().unwrap().deref(), 42);
        assert_eq!(item.unwrap().pop(), 42);

        assert!(queue.peek_mut().is_none());
    }
}
