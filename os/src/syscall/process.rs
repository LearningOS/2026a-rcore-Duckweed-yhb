//! Process management syscalls
use crate::mm::{translated_byte_buffer, PageTable, VirtAddr, PTEFlags};
use crate::task::{
    change_program_brk, current_mmap, current_munmap, current_user_token,
    exit_current_and_run_next, get_syscall_times, suspend_current_and_run_next,
};
use crate::config::PAGE_SIZE;
use crate::timer::get_time_us;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    /// seconds
    pub sec: usize,
    /// microseconds
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// The first invalid virtual address of SV39: 39 bits of address space.
const SV39_LIMIT: usize = 1 << 39;

/// Check that every page of `[addr, addr + len)` is mapped in the address space
/// `token` and allows the requested access (`write` or read).
fn check_user_range(token: usize, addr: usize, len: usize, write: bool) -> bool {
    let page_table = PageTable::from_token(token);
    let end = addr.wrapping_add(len);
    // SV39 only defines 39 useful bits; anything above is not a user address
    if addr >= SV39_LIMIT || end > SV39_LIMIT || end < addr {
        return false;
    }
    let mut cur = addr;
    while cur < end {
        let pte = match page_table.translate(VirtAddr::from(cur).floor()) {
            Some(pte) => pte,
            None => return false,
        };
        if !pte.is_valid() {
            return false;
        }
        let flags = pte.flags();
        let allowed = if write {
            flags.contains(PTEFlags::W)
        } else {
            flags.contains(PTEFlags::R)
        };
        if !allowed {
            return false;
        }
        // move on to the first byte of the next page
        cur = (cur - cur % PAGE_SIZE) + PAGE_SIZE;
    }
    true
}

/// Pointer of one user byte, translated through the page table of `token`.
///
/// `None` means the address is not mapped or does not allow the access.
fn user_byte_ptr(token: usize, addr: usize, write: bool) -> Option<*mut u8> {
    // SV39 only defines 39 useful bits; anything above would alias a real page
    if addr >= SV39_LIMIT {
        return None;
    }
    let page_table = PageTable::from_token(token);
    let pte = page_table.translate(VirtAddr::from(addr).floor())?;
    if !pte.is_valid() {
        return None;
    }
    let flags = pte.flags();
    let allowed = if write {
        flags.contains(PTEFlags::W)
    } else {
        flags.contains(PTEFlags::R)
    };
    if !allowed {
        return None;
    }
    let page = pte.ppn().get_bytes_array();
    Some(unsafe { page.as_mut_ptr().add(addr % PAGE_SIZE) })
}

/// get time with second and microsecond
///
/// The result is written through the page table of the current task, so the
/// destination may even straddle two pages.
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    let value = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    let len = core::mem::size_of::<TimeVal>();
    let src = unsafe { core::slice::from_raw_parts(&value as *const TimeVal as *const u8, len) };
    let token = current_user_token();
    if !check_user_range(token, ts as usize, len, true) {
        return -1;
    }
    let mut copied = 0;
    for buf in translated_byte_buffer(token, ts as *const u8, len) {
        for byte in buf.iter_mut() {
            *byte = src[copied];
            copied += 1;
        }
    }
    0
}

/// trace syscall: read/write one byte of the user address space, or count syscalls
///
/// `trace_request` is 0 for a read, 1 for a write and 2 for a syscall counter
/// query, which is the layout of `TraceRequest` in `user_lib`. Addresses are
/// translated through the page table, so the access mode of the page is
/// respected: writing into a read-only mapping fails with `-1`.
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!(
        "kernel: sys_trace request={} id={:#x} data={}",
        trace_request,
        id,
        data
    );
    let token = current_user_token();
    match trace_request {
        // read one byte from the user address space
        0 => match user_byte_ptr(token, id, false) {
            Some(ptr) => unsafe { *ptr as isize },
            None => -1,
        },
        // write one byte into the user address space
        1 => match user_byte_ptr(token, id, true) {
            Some(ptr) => {
                unsafe { *ptr = data as u8 };
                0
            }
            None => -1,
        },
        // how many times the current task has called syscall `id`
        2 => get_syscall_times(id) as isize,
        _ => -1,
    }
}

/// Map memory at a fixed user virtual address.
pub fn sys_mmap(start: usize, len: usize, port: usize) -> isize {
    trace!("kernel: sys_mmap start={:#x} len={:#x}", start, len);
    if current_mmap(start, len, port) {
        0
    } else {
        -1
    }
}

/// Unmap a user virtual memory area.
pub fn sys_munmap(start: usize, len: usize) -> isize {
    trace!("kernel: sys_munmap start={:#x} len={:#x}", start, len);
    if current_munmap(start, len) {
        0
    } else {
        -1
    }
}

/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
