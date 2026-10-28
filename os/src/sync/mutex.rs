//! Mutex (spin-like and blocking(sleep))

use super::deadlock::{
    current_tid, detect_enabled, task_tid, DEADLOCK_RET,
};
use super::ResKind;
use super::UPSafeCell;
use crate::task::TaskControlBlock;
use crate::task::{
    block_current_and_run_next, current_process, suspend_current_and_run_next,
};
use crate::task::{current_task, wakeup_task};
use alloc::{collections::VecDeque, sync::Arc};

/// Mutex trait
pub trait Mutex: Sync + Send {
    /// Lock the mutex
    ///
    /// Returns [`DEADLOCK_RET`] instead of blocking when the request can never
    /// be satisfied, i.e. only when deadlock detection is enabled for the
    /// current process and the request would close a cycle of waiting threads.
    fn lock(&self) -> isize;
    /// Unlock the mutex
    fn unlock(&self);
}

/// Spinlock Mutex struct
pub struct MutexSpin {
    locked: UPSafeCell<bool>,
}

impl MutexSpin {
    /// Create a new spinlock mutex
    pub fn new() -> Self {
        Self {
            locked: unsafe { UPSafeCell::new(false) },
        }
    }
}

impl Mutex for MutexSpin {
    /// Lock the spinlock mutex
    fn lock(&self) -> isize {
        trace!("kernel: MutexSpin::lock");
        loop {
            let mut locked = self.locked.exclusive_access();
            if *locked {
                drop(locked);
                suspend_current_and_run_next();
                continue;
            } else {
                *locked = true;
                return 0;
            }
        }
    }

    fn unlock(&self) {
        trace!("kernel: MutexSpin::unlock");
        let mut locked = self.locked.exclusive_access();
        *locked = false;
    }
}

/// Blocking Mutex struct
pub struct MutexBlocking {
    /// the id of this mutex in the mutex table of the owning process
    id: usize,
    inner: UPSafeCell<MutexBlockingInner>,
}

pub struct MutexBlockingInner {
    locked: bool,
    wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl MutexBlocking {
    /// Create a new blocking mutex with the given id in the process' table
    pub fn new(id: usize) -> Self {
        trace!("kernel: MutexBlocking::new");
        Self {
            id,
            inner: unsafe {
                UPSafeCell::new(MutexBlockingInner {
                    locked: false,
                    wait_queue: VecDeque::new(),
                })
            },
        }
    }
}

impl Mutex for MutexBlocking {
    /// lock the blocking mutex
    fn lock(&self) -> isize {
        trace!("kernel: MutexBlocking::lock");
        let tid = current_tid();
        // Ask the deadlock detector before sleeping: a request that would make
        // every holder wait forever must be refused instead of blocking.
        if detect_enabled() {
            let would_block = self.inner.exclusive_access().locked;
            if would_block {
                let process = current_process();
                let mut process_inner = process.inner_exclusive_access();
                if process_inner
                    .deadlock
                    .would_deadlock(ResKind::Mutex, self.id, tid)
                {
                    trace!("kernel: MutexBlocking::lock deadlock detected");
                    return DEADLOCK_RET;
                }
                process_inner
                    .deadlock
                    .begin_wait(ResKind::Mutex, self.id, tid);
            }
        }
        let mut mutex_inner = self.inner.exclusive_access();
        if mutex_inner.locked {
            mutex_inner.wait_queue.push_back(current_task().unwrap());
            drop(mutex_inner);
            block_current_and_run_next();
        } else {
            mutex_inner.locked = true;
            drop(mutex_inner);
            if detect_enabled() {
                current_process()
                    .inner_exclusive_access()
                    .deadlock
                    .acquire(ResKind::Mutex, self.id, tid);
            }
        }
        0
    }

    /// unlock the blocking mutex
    fn unlock(&self) {
        trace!("kernel: MutexBlocking::unlock");
        let mut mutex_inner = self.inner.exclusive_access();
        assert!(mutex_inner.locked);
        match mutex_inner.wait_queue.pop_front() {
            Some(waking_task) => {
                drop(mutex_inner);
                // Ownership of the lock moves straight to the woken thread, so
                // it never becomes free in between.
                let tid = current_tid();
                let waking_tid = task_tid(&waking_task);
                if detect_enabled() {
                    let process = current_process();
                    let mut process_inner = process.inner_exclusive_access();
                    process_inner.deadlock.release(ResKind::Mutex, self.id, tid);
                    process_inner
                        .deadlock
                        .acquire(ResKind::Mutex, self.id, waking_tid);
                }
                wakeup_task(waking_task);
            }
            None => {
                mutex_inner.locked = false;
                drop(mutex_inner);
                if detect_enabled() {
                    let tid = current_tid();
                    current_process()
                        .inner_exclusive_access()
                        .deadlock
                        .release(ResKind::Mutex, self.id, tid);
                }
            }
        }
    }
}
