use core::{cell::UnsafeCell, fmt::Debug, mem};

pub struct PeekMut<T: core::fmt::Debug> {
    node: *mut Node<T>,
}

impl<T: core::fmt::Debug> PeekMut<T> {
    pub fn get(&self) -> &T {
        let node = unsafe { &mut *self.node };
        node.get_item()
    }

    pub unsafe fn pop(self) -> *const T {
        Node::remove(self.node);

        (*self.node).get_item() as *const _
    }
}

pub struct List<T: core::fmt::Debug> {
    head: ForwardLink<T>,
}

impl<T: Ord + core::fmt::Debug> List<T> {
    pub fn new() -> Self {
        Self {
            head: ForwardLink::Empty,
        }
    }

    pub unsafe fn insert(&mut self, new_node: *mut Node<T>) {
        let mut prev = BackwardLink::List(self);
        let mut link = self.head.clone();

        loop {
            match link {
                ForwardLink::Empty => {
                    (*new_node).link(prev, ForwardLink::Empty);
                    break;
                }
                ForwardLink::Node(node) => {
                    if (*node).get_item() > (*new_node).get_item() {
                        (*new_node).link(prev, ForwardLink::Node(node));
                        break;
                    } else {
                        prev = BackwardLink::Node(node);
                        link = (*node).get_next().unwrap();
                    }
                }
            }
        }
    }

    pub fn peek_mut(&mut self) -> Option<PeekMut<T>> {
        match self.head {
            ForwardLink::Empty => None,
            ForwardLink::Node(node) => Some(PeekMut { node }),
        }
    }

    pub unsafe fn pop(&mut self) -> Option<*const T> {
        if let Some(peek_mut) = self.peek_mut() {
            Some(peek_mut.pop())
        } else {
            None
        }
    }
}

#[derive(Debug)]
enum ForwardLink<T: core::fmt::Debug> {
    Empty,
    Node(*mut Node<T>),
}

impl<T: core::fmt::Debug> Clone for ForwardLink<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Empty => Self::Empty,
            Self::Node(arg0) => Self::Node(arg0.clone()),
        }
    }
}

#[derive(Debug)]
enum BackwardLink<T: core::fmt::Debug> {
    Node(*mut Node<T>),
    List(*mut List<T>),
}

#[derive(Debug)]
pub enum Node<T: core::fmt::Debug> {
    Unlinked {
        item: T,
    },
    Linked {
        item: T,
        prev: BackwardLink<T>,
        next: ForwardLink<T>,
    },
}

impl<T: core::fmt::Debug> Node<T> {
    pub fn new(item: T) -> Self {
        Self::Unlinked { item }
    }

    unsafe fn link(&mut self, prev_link: BackwardLink<T>, next_link: ForwardLink<T>) {
        match prev_link {
            BackwardLink::Node(node) => match (&mut *node) {
                Node::Unlinked { item } => panic!(),
                Node::Linked { item, next, prev } => *next = next_link.clone(),
            },
            BackwardLink::List(list) => (*list).head = next_link.clone(),
        }

        match next_link {
            ForwardLink::Empty => (),
            ForwardLink::Node(node) => match &mut *node {
                Node::Unlinked { item } => panic!(),
                Node::Linked { item, next, prev } => *prev = prev_link,
            },
        }

        match *self {
            Node::Unlinked { item } => {
                *self = Self::Linked {
                    item,
                    prev: prev_link,
                    next: next_link,
                }
            }
            Node::Linked { item, prev, next } => panic!(),
        }
    }

    fn get_item(&self) -> &T {
        match self {
            Node::Unlinked { item } => item,
            Node::Linked { item, next, prev } => item,
        }
    }

    fn get_next(&self) -> Option<ForwardLink<T>> {
        match self {
            Node::Unlinked { item } => None,
            Node::Linked { item, next, prev } => Some(next.clone()),
        }
    }

    pub unsafe fn remove(node: *mut Self) {
        todo!()
        // match *node {
        //     Node::Unlinked { item } => (),
        //     Node::Linked {
        //         item,
        //         mut next,
        //         mut prev,
        //     } => prev.join(next),
        // }
    }
}

// pub struct List<T> {
//     node: Node<T>,
// }

// impl<T> List<T>
// where
//     T: Ord,
// {
//     pub fn new() -> Self {
//         Self {
//             node: Node::sentinel(),
//         }
//     }

//     pub unsafe fn insert(&mut self, new_node: *mut Node<T>) {
//         Node::insert(&mut self.node, new_node);
//         // self.node.insert(new_node);
//         // let this_ptr = &self.node;

//         // Node::insert(this_ptr, new_node_ptr);
//     }

//     pub fn peek_mut(&mut self) -> Option<PeekMut<T>> {
//         let node = &mut self.node;

//         match node {
//             Node::Sentinel { next } => {
//                 if let Some(next) = *next {
//                     Some(PeekMut { node: next })
//                 } else {
//                     None
//                 }
//             }
//             Node::Entry { prev, next, item } => panic!(),
//         }
//     }

//     pub unsafe fn pop(&mut self) -> Option<*const T> {
//         let node = &mut self.node;

//         match node {
//             Node::Sentinel { next } => {
//                 if let Some(next) = *next {
//                     let peek = PeekMut { node: next };
//                     Some(peek.pop())
//                 } else {
//                     None
//                 }
//             }
//             Node::Entry { prev, next, item } => panic!(),
//         }
//     }
// }

// pub enum Node<T> {
//     Sentinel {
//         next: Option<*mut Node<T>>,
//     },
//     Entry {
//         prev: Option<*mut Node<T>>,
//         next: Option<*mut Node<T>>,
//         item: T,
//     },
// }

// impl<T> Debug for Node<T>
// where
//     T: Debug,
// {
//     fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
//         match self {
//             Self::Sentinel { next } => f.debug_struct("Sentinel").field("next", next).finish(),
//             Self::Entry { prev, next, item } => f
//                 .debug_struct("Entry")
//                 .field("prev", prev)
//                 .field("next", next)
//                 .field("item", item)
//                 .finish(),
//         }
//     }
// }

// impl<T> Node<T> {
//     pub fn new(item: T) -> Self {
//         Self::Entry {
//             prev: None,
//             next: None,
//             item,
//         }
//     }

//     fn sentinel() -> Self {
//         Self::Sentinel { next: None }
//     }

//     fn get_item(&self) -> Option<&T> {
//         if let Node::Entry { prev, next, item } = self {
//             Some(item)
//         } else {
//             None
//         }
//     }

//     fn get_prev_next(&mut self) -> (Option<*mut Self>, Option<*mut Self>) {
//         match self {
//             Node::Sentinel { next } => (None, *next),
//             Node::Entry { prev, next, item } => (*prev, *next),
//         }
//     }

//     unsafe fn join(node: *mut Self, mut next_node: Option<*mut Self>) {
//         // let next_ptr = if let Some(next) = &mut next {
//         //     Some(*next as *mut _)
//         // } else {
//         //     None
//         // };

//         match &mut *node {
//             Node::Sentinel { next } => *next = next_node,
//             Node::Entry { prev, next, item } => *next = next_node,
//         }

//         if let Some(next) = next_node {
//             match &mut *next {
//                 Node::Sentinel { next } => panic!(),
//                 Node::Entry { prev, next, item } => *prev = Some(node),
//             }
//         }
//     }

//     // unsafe fn join(prev_ptr: *mut Self, next_ptr: Option<*mut Self>) {
//     //     let prev_node = unsafe { &mut *prev_ptr };

//     //     match prev_node {
//     //         Node::Sentinel { next } => *next = next_ptr,
//     //         Node::Entry { prev, next, item } => *next = next_ptr,
//     //     }

//     //     if let Some(next_ptr) = next_ptr {
//     //         let next_node = unsafe { &mut *next_ptr };

//     //         match next_node {
//     //             Node::Sentinel { next } => panic!(),
//     //             Node::Entry { prev, next, item } => *prev = Some(prev_ptr),
//     //         }
//     //     }
//     // }

//     pub unsafe fn remove_ptr(node_ptr: *mut Self) {
//         let node = unsafe { &mut *node_ptr };

//         node.remove();
//     }

//     pub unsafe fn remove(&mut self) {
//         if let Node::Entry { prev, next, item } = self {
//             if let Some(prev) = *prev {
//                 let prev = unsafe { &mut *prev };
//                 let next = next.map(|next| next);
//                 Self::join(prev, next);
//             }

//             *prev = None;
//             *next = None;
//         }
//     }
// }

// impl<T> Node<T>
// where
//     T: Ord,
// {
//     unsafe fn insert(node: *mut Self, new_node: *mut Self) {
//         let new_item = match &*new_node {
//             Node::Sentinel { next } => panic!(),
//             Node::Entry { prev, next, item } => item,
//         };

//         loop {
//             let (prev, next) = (&mut *node).get_prev_next();

//             let item = &*item;

//             if prev.is_some() && item > new_item {
//                 let prev = unsafe { &mut *prev.unwrap() };
//                 prev.join(Some(new_node));
//                 (&mut *new_node).join(Some(node));
//                 // Node::join(prev.unwrap(), Some(new_node_ptr));
//                 // Node::join(new_node_ptr, Some(node_ptr));
//                 break;
//             }

//             if next.is_none() {
//                 node.join(Some(new_node));
//                 // Node::join(node_ptr, Some(new_node_ptr));
//                 break;
//             }

//             node = unsafe { &mut *next.unwrap() };

//             // match node {
//             //     Node::Sentinel { next } => {
//             //         let next = *next;

//             //         if next.is_none() {
//             //             node.join(Some(new_node));
//             //             // Node::join(node_ptr, Some(new_node_ptr));
//             //             break;
//             //         }

//             //         node = unsafe { &mut *next.unwrap() };
//             //     }
//             //     Node::Entry { prev, next, item } => {
//             //         let next = *next;
//             //         let item = &*item;

//             //         if item > new_item {
//             //             let prev = unsafe { &mut *prev.unwrap() };
//             //             prev.join(Some(new_node));
//             //             (&mut *new_node).join(Some(node));
//             //             // Node::join(prev.unwrap(), Some(new_node_ptr));
//             //             // Node::join(new_node_ptr, Some(node_ptr));
//             //             break;
//             //         }

//             //         if next.is_none() {
//             //             node.join(Some(new_node));
//             //             // Node::join(node_ptr, Some(new_node_ptr));
//             //             break;
//             //         }

//             //         node = unsafe { &mut *next.unwrap() };
//             //     }
//             // }
//         }
//     }
// }

#[cfg(test)]
mod tests {
    use std::boxed::Box;

    use super::*;
    use std::vec::Vec;

    #[test]
    fn linked_list() {
        let mut root = List::new();
        let list = &mut root;

        let mut items = (0..10)
            .map(|i| Box::into_raw(Box::new(Node::new(i))))
            .collect::<Vec<_>>();

        for item in items {
            let node = item;
            unsafe { list.insert(node) };
        }

        let mut v = Vec::new();

        while let Some(item) = unsafe { list.pop() } {
            let item = unsafe { &*item };
            v.push(*item);
        }

        assert_eq!(v, [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    }
}
