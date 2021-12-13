# Hyperloop

Hyperloop is a priority based async runtime targeting embedded systems written in Rust.

## Project aims
- Provide a lean async runtime suitable for resource constrained microcontrollers.

- No heap allocations.

- No critical sections, using atomics for all synchronization, thus
  being multicore friendly.

- Being a fully fledged alternative to stackful RTOS's with priority based
  scheduling, but with stackless asynchronous tasks.

## Minimum supported Rust version (MSRV)

Hyperloop requires nightly for the time being, due to dependence on unstable features.

## FAQ

### Why async? Can't we just use plain old stackful tasks?

Async tasks have a minimal memory footprint and are a good fit for memory constrained microcontrollers. Tasks should be cheap and you should be able to another task without having to worry much about memory usage. Stackful tasks need a lot of memory for the stack, especially if you need string formatting for logging. Stackful tasks do allow for preemption, but it comes at a high price.

### How does Hyperloop differ from [Embassy](https://github.com/embassy-rs/embassy)

Embassy provides not only an executor, but a whole embedded async ecosystem. Hyperloop is much more limited in scope. The main difference between the Embassy exeutor and Hyperloop is that Hyperloop uses priority based scheduling.
