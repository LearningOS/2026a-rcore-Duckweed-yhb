//! Deadlock detection based on a resource-allocation graph.
//!
//! Every process owns a [`DeadlockGraph`] kept inside its PCB: which threads
//! currently hold which mutex or semaphore, and which resource each blocked
//! thread is waiting for.  Before a thread is put to sleep we look at that
//! graph and ask whether the request could ever be satisfied: a thread waiting
//! for one unit of a resource is only stuck if *every* thread that holds a unit
//! of it is itself stuck.  When no holder is able to run any more, the request
//! can never succeed, so we report a deadlock (`-0xdead`) instead of blocking.

use crate::task::{current_process, current_task, TaskControlBlock};
use alloc::sync::Arc;
use alloc::vec::Vec;

/// The value returned by a blocking synchronization primitive when the request
/// can never be satisfied because it would close a cycle of waiting threads.
pub const DEADLOCK_RET: isize = -0xdead;

/// Kind of a resource tracked by the deadlock detector.
///
/// Mutexes and semaphores live in different id namespaces of a process, so the
/// id alone is not enough to identify a resource.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ResKind {
    /// a mutex
    Mutex,
    /// a semaphore
    Sem,
}

/// Deadlock detector state of one process.
///
/// Detection is disabled by default and switched on by
/// [`crate::syscall::sys_enable_deadlock_detect`], so no bookkeeping is done for
/// processes that never ask for it.
pub struct DeadlockGraph {
    /// whether deadlock detection is enabled for this process
    pub enabled: bool,
    /// threads holding each mutex, indexed by the mutex id of the process
    pub mutex_holders: Vec<Vec<usize>>,
    /// threads holding each semaphore, indexed by the semaphore id of the process
    pub sem_holders: Vec<Vec<usize>>,
    /// the resource each thread is blocked on, indexed by thread id
    pub waiting: Vec<Option<(ResKind, usize)>>,
}

impl DeadlockGraph {
    /// Create an empty graph with detection disabled.
    pub const fn new() -> Self {
        Self {
            enabled: false,
            mutex_holders: Vec::new(),
            sem_holders: Vec::new(),
            waiting: Vec::new(),
        }
    }

    /// The threads that hold one unit of the resource (may contain duplicates
    /// when one thread holds several units of a counting semaphore).
    fn holders(&self, kind: ResKind, id: usize) -> &[usize] {
        let list = match kind {
            ResKind::Mutex => &self.mutex_holders,
            ResKind::Sem => &self.sem_holders,
        };
        match list.get(id) {
            Some(holders) => holders.as_slice(),
            None => &[],
        }
    }

    /// Mutable access to the holder list of a resource, growing it as needed.
    fn holders_mut(&mut self, kind: ResKind, id: usize) -> &mut Vec<usize> {
        let list = match kind {
            ResKind::Mutex => &mut self.mutex_holders,
            ResKind::Sem => &mut self.sem_holders,
        };
        while list.len() <= id {
            list.push(Vec::new());
        }
        &mut list[id]
    }

    /// Record that `tid` acquired one unit of the resource.
    pub fn acquire(&mut self, kind: ResKind, id: usize, tid: usize) {
        let waiting = self.waiting.len();
        if tid < waiting {
            // an acquired resource is never waited for any more
            self.waiting[tid] = None;
        }
        self.holders_mut(kind, id).push(tid);
    }

    /// Record that `tid` released one unit of the resource.
    pub fn release(&mut self, kind: ResKind, id: usize, tid: usize) {
        let holders = self.holders_mut(kind, id);
        if let Some(pos) = holders.iter().position(|&holder| holder == tid) {
            holders.remove(pos);
        }
    }

    /// Record that `tid` is about to block on the resource.
    pub fn begin_wait(&mut self, kind: ResKind, id: usize, tid: usize) {
        while self.waiting.len() <= tid {
            self.waiting.push(None);
        }
        self.waiting[tid] = Some((kind, id));
    }

    /// Forget that `tid` is waiting for anything.
    pub fn clear_waiting(&mut self, tid: usize) {
        if tid < self.waiting.len() {
            self.waiting[tid] = None;
        }
    }

    /// The resource `tid` is blocked on, if any.
    fn waiting_of(&self, tid: usize) -> Option<(ResKind, usize)> {
        self.waiting.get(tid).copied().flatten()
    }

    /// Drop every trace of a thread that is going away, so that a recycled
    /// thread id cannot inherit stale edges of the graph.
    pub fn forget_thread(&mut self, tid: usize) {
        self.clear_waiting(tid);
        for holders in self
            .mutex_holders
            .iter_mut()
            .chain(self.sem_holders.iter_mut())
        {
            holders.retain(|&holder| holder != tid);
        }
    }

    /// Would a request of `tid` for one unit of the resource stay unsatisfied
    /// forever, i.e. would blocking on it close a cycle of waiting threads?
    pub fn would_deadlock(&self, kind: ResKind, id: usize, tid: usize) -> bool {
        !self.can_be_satisfied(kind, id, tid, &mut Vec::new())
    }

    /// Can the request of `tid` for one unit of the resource still be satisfied?
    ///
    /// It can, as soon as one of the current holders is able to run and release
    /// its unit.  A holder that is blocked is itself only stuck if the resource
    /// *it* waits for is stuck, hence the recursion.  `visited` holds the
    /// resources already on the current path so that a cycle which does not lead
    /// back to `tid` terminates instead of looping.
    fn can_be_satisfied(
        &self,
        kind: ResKind,
        id: usize,
        tid: usize,
        visited: &mut Vec<(ResKind, usize)>,
    ) -> bool {
        let holders = self.holders(kind, id);
        if holders.is_empty() {
            // nobody is recorded as a holder: do not claim a deadlock
            return true;
        }
        for &holder in holders {
            if holder == tid {
                // this holder is (transitively) waiting for us, so it cannot
                // give its unit back
                continue;
            }
            match self.waiting_of(holder) {
                // a runnable holder will eventually release the resource
                None => return true,
                Some((next_kind, next_id)) => {
                    if visited.contains(&(next_kind, next_id)) {
                        continue;
                    }
                    visited.push((next_kind, next_id));
                    let satisfiable = self.can_be_satisfied(next_kind, next_id, tid, visited);
                    visited.pop();
                    if satisfiable {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// The thread id of a task.
///
/// A task that has already released its user resources (the exiting task itself)
/// is reported with thread id 0.
pub fn task_tid(task: &Arc<TaskControlBlock>) -> usize {
    task.inner_exclusive_access()
        .res
        .as_ref()
        .map(|res| res.tid)
        .unwrap_or(0)
}

/// The thread id of the current task.
pub fn current_tid() -> usize {
    task_tid(&current_task().unwrap())
}

/// Is deadlock detection enabled for the current process?
pub fn detect_enabled() -> bool {
    current_process()
        .inner_exclusive_access()
        .deadlock
        .enabled
}
