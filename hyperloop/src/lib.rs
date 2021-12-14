#![no_std]
#![feature(const_fn_trait_bound)]
#![feature(type_alias_impl_trait)]
#![feature(once_cell)]

mod common;
mod priority_queue;

pub mod executor;
pub mod interrupt;
pub mod notify;
pub mod task;
pub mod timer;

#[macro_use]
extern crate std;
