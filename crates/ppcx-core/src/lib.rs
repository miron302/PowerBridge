//! ppcx-core: run 32-bit PowerPC Mach-O programs on a modern Mac (Apple silicon or Intel) or any little-endian host.
//!
//! Design: a Mach-O loader + a PowerPC interpreter + "high-level emulation" of the Darwin
//! syscall interface and the most common libSystem imports. No Apple binaries are needed.

pub mod bundle;
pub mod cpu;
pub mod inflate;
pub mod pkg;
pub mod macho;
pub mod machine;
pub mod mem;

mod hle;
mod sys;

pub use machine::{Config, Event, Machine, Outcome, RegSnapshot};
