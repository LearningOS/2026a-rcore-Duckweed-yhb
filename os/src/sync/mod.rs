//! Synchronization and interior mutability primitives

mod condvar;
mod deadlock;
mod mutex;
mod semaphore;
mod up;

pub use condvar::Condvar;
pub use deadlock::{current_tid, detect_enabled, task_tid, DeadlockGraph, ResKind, DEADLOCK_RET};
pub use mutex::{Mutex, MutexBlocking, MutexSpin};
pub use semaphore::Semaphore;
pub use up::UPSafeCell;
