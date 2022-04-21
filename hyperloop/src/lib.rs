#![no_std]
#![feature(const_fn_trait_bound)]
#![feature(type_alias_impl_trait)]
#![feature(once_cell)]
#![feature(option_result_unwrap_unchecked)]

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
