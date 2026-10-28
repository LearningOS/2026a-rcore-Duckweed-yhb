//! Types related to task management

use super::TaskContext;

/// The number of syscalls we keep counters for.
///
/// RISC-V Linux system call numbers stay far below this bound, so a plain array
/// is enough and avoids allocating on every syscall.
pub const MAX_SYSCALL_NUM: usize = 500;

/// The task control block (TCB) of a task.
#[derive(Copy, Clone)]
pub struct TaskControlBlock {
    /// The task status in it's lifecycle
    pub task_status: TaskStatus,
    /// The task context
    pub task_cx: TaskContext,
    /// How many times each syscall has been called by this task, indexed by
    /// the syscall number. Used by `sys_trace` of lab1.
    pub syscall_times: [usize; MAX_SYSCALL_NUM],
}

/// The status of a task
#[derive(Copy, Clone, PartialEq)]
pub enum TaskStatus {
    /// uninitialized
    UnInit,
    /// ready to run
    Ready,
    /// running
    Running,
    /// exited
    Exited,
}
