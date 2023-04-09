#![no_std]

use core::{cell::UnsafeCell, marker::PhantomData, mem, ops::Deref, sync::atomic::Ordering};

#[cfg(not(loom))]
use core::sync::atomic::AtomicUsize;

#[cfg(loom)]
use loom::sync::atomic::AtomicUsize;

use crossbeam_utils::Backoff;

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
where
    T: PartialOrd,
    K: Kind,
{
    heap: &'a PriorityQueue<T, K, N>,
    pos: usize,
    _kind: PhantomData<K>,
}

impl<'a, T, K, const N: usize> Node<'a, T, K, N>
where
    T: PartialOrd + 'static,
    K: Kind + 'static,
{
    fn new(heap: &'a PriorityQueue<T, K, N>, pos: usize) -> Self {
        Self {
            heap,
            pos,
            _kind: PhantomData,
        }
    }

    fn get_node(&self, index: usize) -> Option<Self> {
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
        unsafe { self.heap.slot_mut(self.pos).as_ref().unwrap() }
    }

    unsafe fn slot_mut(&self) -> &mut Option<T> {
        self.heap.slot_mut(self.pos)
    }

    fn swap(self, other: Self) -> Self {
        let slot = unsafe { self.slot_mut() };
        let other_slot = unsafe { other.slot_mut() };

        mem::swap(slot, other_slot);
        other
    }

    fn is_higher_priority(&self, other: &Self) -> bool {
        match K::kind() {
            HeapKind::Max => self.item() > other.item(),
            HeapKind::Min => self.item() < other.item(),
        }
    }
}

struct StackPosition {
    value: usize,
}

impl StackPosition {
    fn from_position(position: usize) -> Self {
        Self {
            value: position << 8,
        }
    }

    fn new(value: usize) -> Self {
        Self { value }
    }

    fn reserved(&self) -> Self {
        assert!(self.value & 0xff != 0xff);

        Self::new(self.value + 1 - (1 << 8))
    }

    fn pushed(&self) -> Self {
        Self::new(self.value - 1)
    }

    fn popped(&self) -> Self {
        Self::new(self.value + (1 << 8))
    }

    fn is_reserved(&self) -> bool {
        (self.value & 0xff) > 0
    }

    fn pos(&self) -> usize {
        self.value >> 8
    }

    fn value(&self) -> usize {
        self.value
    }
}

struct AtomicStackPosition {
    atomic: AtomicUsize,
}

impl AtomicStackPosition {
    fn new(position: usize) -> Self {
        Self {
            atomic: AtomicUsize::new(StackPosition::from_position(position).value()),
        }
    }

    fn load(&self) -> StackPosition {
        StackPosition::new(self.atomic.load(Ordering::Acquire))
    }

    fn compare_exchange(&self, current: usize, new: usize) -> Result<usize, usize> {
        self.atomic
            .compare_exchange_weak(current, new, Ordering::AcqRel, Ordering::Relaxed)
    }
}

pub struct PrioritySender<T>
where
    T: 'static,
{
    slots: *const [UnsafeCell<Option<T>>],
    available: *const AtomicUsize,
    stack_pos: *const AtomicStackPosition,
}

impl<T> Clone for PrioritySender<T> {
    fn clone(&self) -> Self {
        Self {
            slots: self.slots.clone(),
            available: self.available.clone(),
            stack_pos: self.stack_pos.clone(),
        }
    }
}

unsafe impl<T> Send for PrioritySender<T> {}
unsafe impl<T> Sync for PrioritySender<T> {}

impl<T> PrioritySender<T> {
    unsafe fn slot_mut(&self, index: usize) -> &mut Option<T> {
        &mut *(*self.slots)[index].get()
    }

    fn stack_push(&self, item: T) -> Result<(), T> {
        let stack_pos = unsafe { &*self.stack_pos };

        loop {
            let current = stack_pos.load();

            if current.pos() > 0 {
                let new = current.reserved();

                if let Ok(_) = stack_pos.compare_exchange(current.value(), new.value()) {
                    let slot = unsafe { self.slot_mut(new.pos()) };
                    *slot = Some(item);
                    break;
                }
            } else {
                return Err(item);
            }
        }

        loop {
            let old = stack_pos.load();
            let new = old.pushed();

            if let Ok(_) = stack_pos.compare_exchange(old.value(), new.value()) {
                break;
            }
        }

        Ok(())
    }

    pub fn send(&self, item: T) -> Result<(), T> {
        let available = unsafe { &*self.available };

        loop {
            let n_available = available.load(Ordering::Acquire);

            if n_available > 0 {
                if let Ok(_) = available.compare_exchange(
                    n_available,
                    n_available - 1,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    break;
                }
            } else {
                return Err(item);
            }
        }

        self.stack_push(item)
    }
}

pub struct PeekMut<'a, T, K, const N: usize>
where
    T: PartialOrd,
    K: Kind,
{
    queue: &'a mut PriorityQueue<T, K, N>,
}

impl<'a, T, K, const N: usize> PeekMut<'a, T, K, N>
where
    T: PartialOrd + 'static,
    K: Kind + 'static,
{
    pub fn pop(&mut self) -> T {
        self.queue.pop().unwrap()
    }
}

impl<'a, T, K, const N: usize> Deref for PeekMut<'a, T, K, N>
where
    T: PartialOrd + 'static,
    K: Kind + 'static,
{
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { self.queue.slot_mut(0).as_ref().unwrap() }
    }
}

pub struct PriorityQueue<T, K, const N: usize>
where
    T: PartialOrd,
    K: Kind,
{
    slots: [UnsafeCell<Option<T>>; N],
    available: AtomicUsize,
    stack_pos: AtomicStackPosition,
    heap_size: usize,
    _phantom: PhantomData<K>,
}

impl<T, K, const N: usize> PriorityQueue<T, K, N>
where
    T: PartialOrd + 'static,
    K: Kind + 'static,
{
    pub fn new() -> Self {
        Self {
            slots: [(); N].map(|_| UnsafeCell::new(None)),
            available: AtomicUsize::new(N),
            stack_pos: AtomicStackPosition::new(N),
            heap_size: 0,
            _phantom: PhantomData,
        }
    }

    pub unsafe fn get_sender(&self) -> PrioritySender<T> {
        let queue: &'static Self = &*(self as *const Self);

        PrioritySender {
            slots: &queue.slots,
            available: &queue.available,
            stack_pos: &queue.stack_pos,
        }
    }

    unsafe fn slot_mut(&self, index: usize) -> &mut Option<T> {
        &mut *self.slots[index].get()
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

    fn try_stack_pop(&mut self) -> Result<Option<T>, ()> {
        loop {
            let current = self.stack_pos.load();

            if current.pos() == N {
                break Ok(None);
            }

            if current.is_reserved() {
                break Err(());
            } else {
                let new = current.popped();
                let item = unsafe { self.slot_mut(current.pos()).take() };

                if let Ok(_) = self
                    .stack_pos
                    .compare_exchange(current.value(), new.value())
                {
                    break Ok(item);
                } else {
                    unsafe {
                        *self.slot_mut(current.pos()) = item;
                    }
                }
            }
        }
    }

    fn stack_pop(&mut self) -> Option<T> {
        let backoff = Backoff::new();

        loop {
            if let Ok(item) = self.try_stack_pop() {
                break item;
            }

            backoff.spin();
        }
    }

    fn move_to_heap(&mut self) -> Result<(), ()> {
        if let Some(item) = self.stack_pop() {
            let _ = self.heap_insert(item);
            Ok(())
        } else {
            Err(())
        }
    }

    fn sort(&mut self) {
        while self.move_to_heap().is_ok() {}
    }

    fn heap_insert(&mut self, item: T) -> Result<(), T> {
        let index = self.heap_size;

        if index < N {
            unsafe {
                *self.slot_mut(index) = Some(item);
            }

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
        if self.heap_size > 1 {
            let item = unsafe { self.slot_mut(0).take() }.unwrap();
            {
                let root = self.get_root();
                let last = self.get_last();
                last.swap(root);
            }
            self.heap_size -= 1;

            Some(item)
        } else if self.heap_size == 1 {
            self.heap_size -= 1;
            Some(unsafe { self.slot_mut(0).take() }.unwrap())
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
                let available = self.available.load(Ordering::Acquire);

                if let Ok(_) = self.available.compare_exchange(
                    available,
                    available + 1,
                    Ordering::Release,
                    Ordering::Relaxed,
                ) {
                    break Some(item);
                }
            }
        } else {
            None
        }
    }

    pub fn peek_mut<'a>(&'a mut self) -> Option<PeekMut<'a, T, K, N>> {
        self.sort();

        if self.heap_size > 0 {
            Some(PeekMut { queue: self })
        } else {
            None
        }
    }
}

#[cfg(not(loom))]
#[cfg(test)]
mod tests {
    use std::{thread, vec::Vec};

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
        let sender = unsafe { heap.get_sender() };

        for i in 0..10 {
            sender.stack_push(i).unwrap();
        }

        assert!(sender.stack_push(11).is_err());

        for i in (0..10).rev() {
            assert_eq!(heap.stack_pop(), Some(i));
        }

        assert!(heap.stack_pop().is_none());

        for i in 0..5 {
            sender.stack_push(i).unwrap();
        }

        for i in (0..5).rev() {
            assert_eq!(heap.stack_pop(), Some(i));
        }

        assert!(heap.stack_pop().is_none());
    }

    #[test]
    fn channel() {
        let mut queue: PriorityQueue<u32, Min, 10> = PriorityQueue::new();
        let sender = unsafe { queue.get_sender() };

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

    #[test]
    #[cfg_attr(miri, ignore)]
    fn channel_thread() {
        const N: usize = 1000;
        let mut queue: PriorityQueue<u128, Min, N> = PriorityQueue::new();
        let mut handlers = Vec::new();
        let mut items = Vec::new();

        let n_threads = 10;
        let n_items_per_thread = 1000;
        let n_items = n_threads * n_items_per_thread;

        for i in 0..n_threads {
            let sender = unsafe { queue.get_sender() };
            let handler = thread::spawn(move || {
                for j in 0..n_items_per_thread {
                    loop {
                        let item = i * n_items_per_thread + j;

                        if let Ok(_) = sender.send(item) {
                            break;
                        }

                        std::thread::sleep(core::time::Duration::from_nanos(1));
                    }
                }
            });
            handlers.push(handler);
        }

        for _ in 0..n_items {
            loop {
                if let Some(item) = queue.pop() {
                    items.push(item);
                    break;
                }

                std::thread::sleep(core::time::Duration::from_nanos(1));
            }
        }

        for handler in handlers {
            handler.join().unwrap();
        }

        items.sort();

        for i in (0..n_items).rev() {
            assert_eq!(items.pop(), Some(i));
        }
    }
}

#[cfg(test)]
#[macro_use]
extern crate std;

#[cfg(test)]
#[cfg(loom)]
mod tests_loom {
    use std::boxed::Box;
    use std::vec::Vec;

    use loom::thread;

    use super::*;

    #[test]
    fn stack() {
        loom::model(|| {
            let queue: &'static mut PriorityQueue<u128, Min, 2> =
                Box::leak(Box::new(PriorityQueue::new()));

            let n_threads = 2;

            let handles: Vec<_> = (0..n_threads)
                .map(|i| {
                    let sender = queue.get_sender();
                    thread::spawn(move || {
                        sender.stack_push(i).unwrap();
                    })
                })
                .collect();

            let consumer = thread::spawn(move || {
                let item = queue.try_stack_pop();
                (item, queue)
            });

            for handle in handles {
                handle.join().unwrap();
            }
            let (item, queue) = consumer.join().unwrap();

            let first = if let Ok(Some(value)) = item {
                value
            } else {
                queue.try_stack_pop().unwrap().unwrap()
            };

            let next = if first == 0 { 1 } else { 0 };

            assert_eq!(queue.try_stack_pop(), Ok(Some(next)));
            assert_eq!(queue.try_stack_pop(), Ok(None));
        });
    }
}
