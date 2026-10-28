//! Process management syscalls
use crate::{
    task::{exit_current_and_run_next, get_syscall_times, suspend_current_and_run_next},
    timer::get_time_us,
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    /// seconds
    pub sec: usize,
    /// microseconds
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(exit_code: i32) -> ! {
    trace!("[kernel] Application exited with code {}", exit_code);
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// get time with second and microsecond
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    unsafe {
        *ts = TimeVal {
            sec: us / 1_000_000,
            usec: us % 1_000_000,
        };
    }
    0
}

/// trace syscall: read/write the user address space, or count syscalls
///
/// `trace_request` is 0 for a read, 1 for a write and 2 for a syscall counter
/// query, which is the layout of `TraceRequest` in `user_lib`.
///
/// Chapter 3 has no address space isolation: a user address is a physical
/// address and can be dereferenced directly by the kernel.
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!(
        "kernel: sys_trace request={} id={:#x} data={}",
        trace_request,
        id,
        data
    );
    match trace_request {
        // read one byte from the user address space
        0 => unsafe { (id as *const u8).read_volatile() as isize },
        // write one byte into the user address space
        1 => {
            unsafe { (id as *mut u8).write_volatile(data as u8) };
            0
        }
        // how many times the current task has called syscall `id`
        2 => get_syscall_times(id) as isize,
        _ => -1,
    }
}
