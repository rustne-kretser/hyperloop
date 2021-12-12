#![no_std]
#![feature(const_fn_trait_bound)]
#![feature(type_alias_impl_trait)]
#![feature(once_cell)]

extern crate alloc;

mod priority_queue;
mod common;

pub mod timer;
pub mod notify;
pub mod interrupt;
pub mod task;
pub mod executor;

#[macro_use]
extern crate std;

