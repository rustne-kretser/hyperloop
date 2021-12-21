#![no_std]
#![feature(const_fn_trait_bound)]
#![feature(type_alias_impl_trait)]
#![feature(once_cell)]

mod common;

pub mod executor;
pub mod interrupt;
pub mod notify;
pub mod task;
pub mod timer;

mod priority_queue {
    pub(crate) use hyperloop_priority_queue::*;
}

#[macro_use]
extern crate std;

#[cfg(test)]
mod tests {
    use crate::executor::Executor;
    use crate::task::Task;
    use crossbeam_queue::ArrayQueue;
    use hyperloop_macros::{executor_from_tasks, task};
    use std::sync::Arc;

    #[test]
    fn macros() {
        #[task(priority = 1)]
        async fn test_task1(queue: Arc<ArrayQueue<u32>>) {
            queue.push(1).unwrap();
        }

        #[task(priority = 2)]
        async fn test_task2(queue: Arc<ArrayQueue<u32>>) {
            queue.push(2).unwrap();
        }

        let queue = Arc::new(ArrayQueue::new(10));

        let task1 = test_task1(queue.clone()).unwrap();
        let task2 = test_task2(queue.clone()).unwrap();

        let mut executor = executor_from_tasks!(task1, task2);

        executor.poll_tasks();

        assert_eq!(queue.pop().unwrap(), 2);
        assert_eq!(queue.pop().unwrap(), 1);
    }
}
