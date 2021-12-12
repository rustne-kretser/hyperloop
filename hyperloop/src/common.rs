#[cfg(test)]
pub mod tests {
    use core::sync::atomic::{AtomicBool, Ordering};

    use std::{sync::Arc, task::Wake};

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

    use log::{Record, Level, Metadata};

    struct SimpleLogger;

    use log::LevelFilter;

    static LOGGER: SimpleLogger = SimpleLogger;

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

    pub fn log_init() {
        let _ = log::set_logger(&LOGGER)
            .map(|()| log::set_max_level(LevelFilter::Info));
    }
}
