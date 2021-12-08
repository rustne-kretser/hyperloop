# Hyperloop – A priority based async runtime targeting embedded systems

When the superloop isn't so super anymore and stackfull RTOS tasks eat
up all your RAM, look no further, Hyperloop is here to help.

Hyperloop has the following aims:

- Provide a lean async runtime suitable for resource constrained microcontrollers.

- No heap allocations.

- No critical sections, using atomics for all synchronization, thus
  being multicore friendly.

- Being a fully fledged alternative to stackfull RTOS's with priority based
  scheduling, but with stackless asynchronous tasks.

## Minimum supported Rust version (MSRV)

Hyperloop requires nightly for the time being, due to dependence on unstable features.
