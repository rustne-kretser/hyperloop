#[cfg(test)]
pub mod tests {
    use core::sync::atomic::{AtomicBool, Ordering};

    use alloc::{sync::Arc, task::Wake};

    pub struct MockWaker {
        pub woke: AtomicBool,
    }

    impl MockWaker {
        pub fn new() -> Self {
            Self { woke: AtomicBool::new(false) }
        }
    }

    impl Wake for MockWaker {
        fn wake(self: Arc<Self>) {
            self.woke.store(true, Ordering::Relaxed);
        }
    }
}
