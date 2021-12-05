use core::task::{RawWaker, RawWakerVTable, Waker};

use crate::executor::Task;

unsafe fn clone(ptr: *const ()) -> RawWaker {
    RawWaker::new(ptr, &VTABLE)
}

unsafe fn wake(ptr: *const ()) {
    let task = &*(ptr as *const Task);
    task.wake();
}

unsafe fn drop(_ptr: *const ()) {
}

const VTABLE: RawWakerVTable = RawWakerVTable::new(clone,
                                                   wake,
                                                   wake,
                                                   drop);

pub fn get_waker(task: &Task) -> Waker {
    let ptr: *const () = (task as *const Task).cast();

    unsafe {
        Waker::from_raw(RawWaker::new(ptr, &VTABLE))
    }
}
