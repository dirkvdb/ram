use std::{collections::HashMap, ffi::c_void, io, mem, path::Path};

use crate::{Memory, PlatformDetails, ProcessGroup, Snapshot, rank_process_groups};

type Bool = i32;
type Dword = u32;
type Handle = *mut c_void;

const PROCESS_VM_READ: Dword = 0x0010;
const PROCESS_QUERY_INFORMATION: Dword = 0x0400;
const STD_OUTPUT_HANDLE: Dword = (-11_i32) as Dword;
const ENABLE_VIRTUAL_TERMINAL_PROCESSING: Dword = 0x0004;

#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct MemoryStatusEx {
    length: Dword,
    memory_load: Dword,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended_virtual: u64,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PerformanceInformation {
    cb: Dword,
    commit_total: usize,
    commit_limit: usize,
    commit_peak: usize,
    physical_total: usize,
    physical_available: usize,
    system_cache: usize,
    kernel_total: usize,
    kernel_paged: usize,
    kernel_nonpaged: usize,
    page_size: usize,
    handle_count: Dword,
    process_count: Dword,
    thread_count: Dword,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ProcessMemoryCounters {
    cb: Dword,
    page_fault_count: Dword,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_nonpaged_pool_usage: usize,
    quota_nonpaged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    #[link_name = "GlobalMemoryStatusEx"]
    fn global_memory_status_ex(buffer: *mut MemoryStatusEx) -> Bool;
    #[link_name = "GetComputerNameW"]
    fn get_computer_name(buffer: *mut u16, size: *mut Dword) -> Bool;
    #[link_name = "OpenProcess"]
    fn open_process(access: Dword, inherit_handle: Bool, process_id: Dword) -> Handle;
    #[link_name = "CloseHandle"]
    fn close_handle(handle: Handle) -> Bool;
    #[link_name = "QueryFullProcessImageNameW"]
    fn query_full_process_image_name(
        process: Handle,
        flags: Dword,
        path: *mut u16,
        size: *mut Dword,
    ) -> Bool;
    #[link_name = "GetStdHandle"]
    fn get_std_handle(std_handle: Dword) -> Handle;
    #[link_name = "GetConsoleMode"]
    fn get_console_mode(console: Handle, mode: *mut Dword) -> Bool;
    #[link_name = "SetConsoleMode"]
    fn set_console_mode(console: Handle, mode: Dword) -> Bool;
}

#[link(name = "psapi")]
unsafe extern "system" {
    #[link_name = "GetPerformanceInfo"]
    fn get_performance_info(info: *mut PerformanceInformation, size: Dword) -> Bool;
    #[link_name = "EnumProcesses"]
    fn enum_processes(process_ids: *mut Dword, bytes: Dword, bytes_used: *mut Dword) -> Bool;
    #[link_name = "GetProcessMemoryInfo"]
    fn get_process_memory_info(
        process: Handle,
        counters: *mut ProcessMemoryCounters,
        size: Dword,
    ) -> Bool;
}

struct OwnedHandle(Handle);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        // SAFETY: The handle was returned by OpenProcess and is owned here.
        unsafe {
            close_handle(self.0);
        }
    }
}

pub(crate) fn collect(prettify: bool, process_count: usize) -> io::Result<Snapshot> {
    let status = memory_status()?;
    let performance = performance_info()?;
    let page_size = performance.page_size as u64;

    Ok(Snapshot {
        hostname: hostname(),
        memory: Memory {
            total: status.total_phys,
            available: status.avail_phys.min(status.total_phys),
            cached: pages(performance.system_cache, page_size),
            commit_limit: pages(performance.commit_limit, page_size),
            committed: pages(performance.commit_total, page_size),
            ..Memory::default()
        },
        details: PlatformDetails::Windows,
        groups: process_groups(prettify, process_count),
    })
}

fn memory_status() -> io::Result<MemoryStatusEx> {
    let mut status = MemoryStatusEx {
        length: mem::size_of::<MemoryStatusEx>() as Dword,
        ..MemoryStatusEx::default()
    };
    // SAFETY: `status` is writable and its required length field is initialized.
    if unsafe { global_memory_status_ex(&raw mut status) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(status)
    }
}

fn performance_info() -> io::Result<PerformanceInformation> {
    let mut info = PerformanceInformation {
        cb: mem::size_of::<PerformanceInformation>() as Dword,
        ..PerformanceInformation::default()
    };
    // SAFETY: `info` is a writable PERFORMANCE_INFORMATION-sized buffer.
    if unsafe { get_performance_info(&raw mut info, info.cb) } == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(info)
    }
}

fn process_groups(prettify: bool, process_count: usize) -> Vec<ProcessGroup> {
    let mut groups: HashMap<String, (usize, u64)> = HashMap::new();

    for pid in pids() {
        let Some(process) = open_process_for_query(pid) else {
            continue;
        };
        let Some(rss) = process_memory(process.0) else {
            continue;
        };
        if rss == 0 {
            continue;
        }
        let Some(executable) = process_name(process.0, prettify) else {
            continue;
        };
        let item = groups.entry(executable).or_insert((0, 0));
        item.0 += 1;
        item.1 = item.1.saturating_add(rss);
    }

    rank_process_groups(groups, process_count)
}

fn pids() -> Vec<Dword> {
    let mut capacity = 1024_usize;
    loop {
        let mut process_ids = vec![0; capacity];
        let Ok(buffer_bytes) = Dword::try_from(process_ids.len() * mem::size_of::<Dword>()) else {
            return Vec::new();
        };
        let mut bytes_used = 0;
        // SAFETY: `process_ids` is a writable buffer of `buffer_bytes` bytes.
        if unsafe { enum_processes(process_ids.as_mut_ptr(), buffer_bytes, &raw mut bytes_used) }
            == 0
        {
            return Vec::new();
        }
        if bytes_used < buffer_bytes {
            process_ids.truncate(bytes_used as usize / mem::size_of::<Dword>());
            process_ids.retain(|pid| *pid != 0);
            return process_ids;
        }
        capacity = capacity.saturating_mul(2);
        if capacity > 1_048_576 {
            return Vec::new();
        }
    }
}

fn open_process_for_query(pid: Dword) -> Option<OwnedHandle> {
    // SAFETY: OpenProcess does not dereference pointers and returns an owned handle.
    let handle = unsafe { open_process(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, pid) };
    (!handle.is_null()).then_some(OwnedHandle(handle))
}

fn process_memory(process: Handle) -> Option<u64> {
    let mut counters = ProcessMemoryCounters {
        cb: mem::size_of::<ProcessMemoryCounters>() as Dword,
        ..ProcessMemoryCounters::default()
    };
    // SAFETY: `counters` is a writable PROCESS_MEMORY_COUNTERS-sized buffer and
    // `process` remains open for the duration of the call.
    if unsafe { get_process_memory_info(process, &raw mut counters, counters.cb) } == 0 {
        None
    } else {
        u64::try_from(counters.working_set_size).ok()
    }
}

fn process_name(process: Handle, prettify: bool) -> Option<String> {
    let mut path = vec![0_u16; 32_768];
    let mut length = path.len() as Dword;
    // SAFETY: `path` is writable for `length` UTF-16 code units and `process`
    // remains open for the duration of the call.
    if unsafe { query_full_process_image_name(process, 0, path.as_mut_ptr(), &raw mut length) } == 0
    {
        return None;
    }
    path.truncate(length as usize);
    let path = String::from_utf16_lossy(&path);
    let name = path
        .rsplit(['\\', '/'])
        .next()
        .filter(|name| !name.is_empty())?;
    Some(if prettify {
        crate::clean_executable_name(Path::new(&path), name)
    } else {
        crate::sanitize_name(name)
    })
}

fn hostname() -> String {
    let mut buffer = [0_u16; 256];
    let mut length = buffer.len() as Dword;
    // SAFETY: `buffer` is writable for `length` UTF-16 code units.
    if unsafe { get_computer_name(buffer.as_mut_ptr(), &raw mut length) } != 0 && length > 0 {
        return crate::sanitize_name(&String::from_utf16_lossy(&buffer[..length as usize]));
    }
    "windows".to_string()
}

fn pages(count: usize, page_size: u64) -> u64 {
    u64::try_from(count)
        .unwrap_or(u64::MAX)
        .saturating_mul(page_size)
}

pub(crate) fn enable_virtual_terminal_processing() -> bool {
    // SAFETY: These calls only query and update mode flags on the stdout handle.
    unsafe {
        let console = get_std_handle(STD_OUTPUT_HANDLE);
        if console.is_null() || console == (-1_isize) as Handle {
            return false;
        }
        let mut mode = 0;
        get_console_mode(console, &raw mut mode) != 0
            && set_console_mode(console, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}
