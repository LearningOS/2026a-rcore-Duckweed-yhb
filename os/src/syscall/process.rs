//! Process management syscalls
//!
use alloc::sync::Arc;

use crate::{
    config::PAGE_SIZE,
    fs::{open_file, OpenFlags},
    mm::{
        translated_byte_buffer, translated_refmut, translated_str, MapPermission, VirtAddr,
        VirtPageNum,
    },
    task::{
        add_task, current_task, current_user_token, exit_current_and_run_next,
        suspend_current_and_run_next,
    },
    timer::get_time_us,
};

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

pub fn sys_exit(exit_code: i32) -> ! {
    trace!("kernel:pid[{}] sys_exit", current_task().unwrap().pid.0);
    exit_current_and_run_next(exit_code);
    panic!("Unreachable in sys_exit!");
}

pub fn sys_yield() -> isize {
    //trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

pub fn sys_getpid() -> isize {
    trace!("kernel: sys_getpid pid:{}", current_task().unwrap().pid.0);
    current_task().unwrap().pid.0 as isize
}

pub fn sys_fork() -> isize {
    trace!("kernel:pid[{}] sys_fork", current_task().unwrap().pid.0);
    let current_task = current_task().unwrap();
    let new_task = current_task.fork();
    let new_pid = new_task.pid.0;
    // modify trap context of new_task, because it returns immediately after switching
    let trap_cx = new_task.inner_exclusive_access().get_trap_cx();
    // we do not have to move to next instruction since we have done it before
    // for child process, fork returns 0
    trap_cx.x[10] = 0;
    // add new task to scheduler
    add_task(new_task);
    new_pid as isize
}

pub fn sys_exec(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_exec", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
        let task = current_task().unwrap();
        task.exec(all_data.as_slice());
        0
    } else {
        -1
    }
}

/// If there is not a child process whose pid is same as given, return -1.
/// Else if there is a child process but it is still running, return -2.
pub fn sys_waitpid(pid: isize, exit_code_ptr: *mut i32) -> isize {
    //trace!("kernel: sys_waitpid");
    let task = current_task().unwrap();
    // find a child process

    // ---- access current PCB exclusively
    let mut inner = task.inner_exclusive_access();
    if !inner
        .children
        .iter()
        .any(|p| pid == -1 || pid as usize == p.getpid())
    {
        return -1;
        // ---- release current PCB
    }
    let pair = inner.children.iter().enumerate().find(|(_, p)| {
        // ++++ temporarily access child PCB exclusively
        p.inner_exclusive_access().is_zombie() && (pid == -1 || pid as usize == p.getpid())
        // ++++ release child PCB
    });
    if let Some((idx, _)) = pair {
        let child = inner.children.remove(idx);
        // confirm that child will be deallocated after being removed from children list
        assert_eq!(Arc::strong_count(&child), 1);
        let found_pid = child.getpid();
        // ++++ temporarily access child PCB exclusively
        let exit_code = child.inner_exclusive_access().exit_code;
        // ++++ release child PCB
        *translated_refmut(inner.memory_set.token(), exit_code_ptr) = exit_code;
        found_pid as isize
    } else {
        -2
    }
    // ---- release current PCB automatically
}

/// Get the current time with second and microsecond into the user struct `ts`
///
/// The struct may be split by two pages, so it is written byte by byte
/// through the page table instead of through a raw pointer.
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel:pid[{}] sys_get_time", current_task().unwrap().pid.0);
    let token = current_user_token();
    let time_us = get_time_us();
    let time_val = TimeVal {
        sec: time_us / 1_000_000,
        usec: time_us % 1_000_000,
    };
    // copy the bytes of `time_val` into user space
    let size = core::mem::size_of::<TimeVal>();
    let src = unsafe {
        core::slice::from_raw_parts(&time_val as *const TimeVal as *const u8, size)
    };
    let mut copied = 0usize;
    for slice in translated_byte_buffer(token, ts as *const u8, size).iter_mut() {
        let len = slice.len();
        slice.copy_from_slice(&src[copied..copied + len]);
        copied += len;
    }
    0
}

/// Map the virtual page range `[start, start + len)` with the permissions in `port`
///
/// `port` is a bit set: bit 0 means readable, bit 1 writable and bit 2 executable.
/// Return -1 if the arguments are invalid or the range is mapped before.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    trace!("kernel:pid[{}] sys_mmap", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    // the start address must be page aligned, and only R/W/X bits are allowed
    if start % PAGE_SIZE != 0 || len == 0 || port == 0 || port & !0x7 != 0 {
        return -1;
    }
    let start_vpn: VirtPageNum = VirtAddr::from(start).into();
    let end_vpn: VirtPageNum = VirtAddr::from(start + len).ceil();
    if end_vpn <= start_vpn || !inner.memory_set.is_vpn_range_free(start_vpn, end_vpn) {
        return -1;
    }
    let mut map_perm = MapPermission::U;
    if port & 0x1 != 0 {
        map_perm |= MapPermission::R;
    }
    if port & 0x2 != 0 {
        map_perm |= MapPermission::W;
    }
    if port & 0x4 != 0 {
        map_perm |= MapPermission::X;
    }
    inner
        .memory_set
        .insert_framed_area(start.into(), (start + len).into(), map_perm);
    0
}

/// Unmap the virtual page range `[start, start + len)`
///
/// The whole range must be page aligned and exactly equal to an area mapped
/// before, otherwise -1 is returned.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel:pid[{}] sys_munmap", current_task().unwrap().pid.0);
    let task = current_task().unwrap();
    let mut inner = task.inner_exclusive_access();
    if start % PAGE_SIZE != 0 || len == 0 || len % PAGE_SIZE != 0 {
        return -1;
    }
    let start_vpn: VirtPageNum = VirtAddr::from(start).into();
    let end_vpn: VirtPageNum = VirtAddr::from(start + len).into();
    if inner.memory_set.remove_area(start_vpn, end_vpn) {
        0
    } else {
        -1
    }
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel:pid[{}] sys_sbrk", current_task().unwrap().pid.0);
    if let Some(old_brk) = current_task().unwrap().change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}

/// Create a new process by loading the elf file `path`, and return its pid
pub fn sys_spawn(path: *const u8) -> isize {
    trace!("kernel:pid[{}] sys_spawn", current_task().unwrap().pid.0);
    let token = current_user_token();
    let path = translated_str(token, path);
    if let Some(app_inode) = open_file(path.as_str(), OpenFlags::RDONLY) {
        let all_data = app_inode.read_all();
        let task = current_task().unwrap();
        let new_task = task.spawn(all_data.as_slice());
        let new_pid = new_task.pid.0;
        // add new task to scheduler
        add_task(new_task);
        new_pid as isize
    } else {
        -1
    }
}

// YOUR JOB: Set task priority.
pub fn sys_set_priority(_prio: isize) -> isize {
    trace!(
        "kernel:pid[{}] sys_set_priority NOT IMPLEMENTED",
        current_task().unwrap().pid.0
    );
    -1
}
