//! Semaphore

use crate::sync::deadlock::{current_tid, detect_enabled, task_tid, DEADLOCK_RET};
use crate::sync::{ResKind, UPSafeCell};
use crate::task::{
    block_current_and_run_next, current_process, current_task, wakeup_task, TaskControlBlock,
};
use alloc::{collections::VecDeque, sync::Arc};

/// semaphore structure
pub struct Semaphore {
    /// the id of this semaphore in the semaphore table of the owning process
    id: usize,
    /// semaphore inner
    pub inner: UPSafeCell<SemaphoreInner>,
}

pub struct SemaphoreInner {
    pub count: isize,
    pub wait_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl Semaphore {
    /// Create a new semaphore with the given id in the process' table
    pub fn new(res_count: usize, id: usize) -> Self {
        trace!("kernel: Semaphore::new");
        Self {
            id,
            inner: unsafe {
                UPSafeCell::new(SemaphoreInner {
                    count: res_count as isize,
                    wait_queue: VecDeque::new(),
                })
            },
        }
    }

    /// up operation of semaphore
    pub fn up(&self) {
        trace!("kernel: Semaphore::up");
        let detect = detect_enabled();
        let tid = if detect { current_tid() } else { 0 };
        let mut inner = self.inner.exclusive_access();
        inner.count += 1;
        if inner.count <= 0 {
            if let Some(task) = inner.wait_queue.pop_front() {
                let waking_tid = task_tid(&task);
                drop(inner);
                if detect {
                    // the released unit is handed over to the woken thread
                    let process = current_process();
                    let mut process_inner = process.inner_exclusive_access();
                    process_inner.deadlock.release(ResKind::Sem, self.id, tid);
                    process_inner
                        .deadlock
                        .acquire(ResKind::Sem, self.id, waking_tid);
                }
                wakeup_task(task);
            }
        } else {
            drop(inner);
            if detect {
                current_process()
                    .inner_exclusive_access()
                    .deadlock
                    .release(ResKind::Sem, self.id, tid);
            }
        }
    }

    /// down operation of semaphore
    ///
    /// Returns [`DEADLOCK_RET`] instead of blocking when the request can never
    /// be satisfied and deadlock detection is enabled for the current process.
    pub fn down(&self) -> isize {
        trace!("kernel: Semaphore::down");
        let detect = detect_enabled();
        if detect {
            let tid = current_tid();
            let would_block = self.inner.exclusive_access().count <= 0;
            if would_block {
                let process = current_process();
                let mut process_inner = process.inner_exclusive_access();
                if process_inner
                    .deadlock
                    .would_deadlock(ResKind::Sem, self.id, tid)
                {
                    trace!("kernel: Semaphore::down deadlock detected");
                    return DEADLOCK_RET;
                }
                process_inner
                    .deadlock
                    .begin_wait(ResKind::Sem, self.id, tid);
            }
        }
        let mut inner = self.inner.exclusive_access();
        inner.count -= 1;
        if inner.count < 0 {
            inner.wait_queue.push_back(current_task().unwrap());
            drop(inner);
            block_current_and_run_next();
        } else {
            drop(inner);
            if detect {
                current_process()
                    .inner_exclusive_access()
                    .deadlock
                    .acquire(ResKind::Sem, self.id, current_tid());
            }
        }
        0
    }
}
