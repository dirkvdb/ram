use std::{
    collections::HashMap,
    ffi::{CStr, CString, c_char, c_int, c_void},
    io,
    mem::{self, MaybeUninit},
    path::Path,
    ptr,
};

use crate::{
    DEFAULT_PAGE_SIZE, Memory, PlatformDetails, PressureLevel, ProcessGroup, Snapshot,
    rank_process_groups,
};

const HOST_VM_INFO64: c_int = 4;
// offsetof(vm_statistics64, swapped_count) / sizeof(integer_t)
const HOST_VM_INFO64_REV1_COUNT: u32 = 38;
const PROC_ALL_PIDS: u32 = 1;
const PROC_PIDTASKINFO: c_int = 4;
const PROC_PIDPATHINFO_MAXSIZE: u32 = 4096;
const RUSAGE_INFO_V2: c_int = 2;
const KERN_SUCCESS: c_int = 0;

#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct VmStatistics64 {
    free_count: u32,
    active_count: u32,
    inactive_count: u32,
    wire_count: u32,
    zero_fill_count: u64,
    reactivations: u64,
    pageins: u64,
    pageouts: u64,
    faults: u64,
    cow_faults: u64,
    lookups: u64,
    hits: u64,
    purges: u64,
    purgeable_count: u32,
    speculative_count: u32,
    decompressions: u64,
    compressions: u64,
    swapins: u64,
    swapouts: u64,
    compressor_page_count: u32,
    throttled_count: u32,
    external_page_count: u32,
    internal_page_count: u32,
    total_uncompressed_pages_in_compressor: u64,
}

#[allow(dead_code)]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct XswUsage {
    xsu_total: u64,
    xsu_avail: u64,
    xsu_used: u64,
    xsu_pagesize: u32,
    xsu_encrypted: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ProcTaskInfo {
    pti_virtual_size: u64,
    pti_resident_size: u64,
    _rest: [u8; 80],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct RusageInfoV2 {
    _ri_uuid: [u8; 16],
    _ri_user_time: u64,
    _ri_system_time: u64,
    _ri_pkg_idle_wkups: u64,
    _ri_interrupt_wkups: u64,
    _ri_pageins: u64,
    _ri_wired_size: u64,
    _ri_resident_size: u64,
    ri_phys_footprint: u64,
    _tail: [u64; 10],
}

unsafe extern "C" {
    fn mach_host_self() -> u32;
    fn host_page_size(host: u32, page_size: *mut usize) -> c_int;
    fn host_statistics64(host: u32, flavor: c_int, info: *mut i32, count: *mut u32) -> c_int;
    fn sysctlbyname(
        name: *const c_char,
        oldp: *mut c_void,
        oldlenp: *mut usize,
        newp: *mut c_void,
        newlen: usize,
    ) -> c_int;
    fn gethostname(name: *mut c_char, namelen: usize) -> c_int;
    fn proc_listpids(type_: u32, typeinfo: u32, buffer: *mut c_void, buffersize: c_int) -> c_int;
    fn proc_pidinfo(
        pid: c_int,
        flavor: c_int,
        arg: u64,
        buffer: *mut c_void,
        buffersize: c_int,
    ) -> c_int;
    fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    fn proc_name(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
    fn proc_pid_rusage(pid: c_int, flavor: c_int, buffer: *mut c_void) -> c_int;
}

pub(crate) fn collect(prettify: bool, process_count: usize) -> io::Result<Snapshot> {
    let page_size = page_size();
    let total = sysctl::<u64>("hw.memsize")?;
    let vm = vm_statistics()?;
    let swap = sysctl::<XswUsage>("vm.swapusage").unwrap_or_default();
    let pressure = sysctl::<i32>("kern.memorystatus_vm_pressure_level")
        .map(PressureLevel::from_sysctl)
        .unwrap_or(PressureLevel::Unknown(-1));

    let app = pages(
        vm.internal_page_count.saturating_sub(vm.purgeable_count),
        page_size,
    );
    let wired = pages(vm.wire_count, page_size);
    let compressor_ram = pages(vm.compressor_page_count, page_size);
    let used = app
        .saturating_add(wired)
        .saturating_add(compressor_ram)
        .min(total);
    let cached = pages(vm.external_page_count, page_size);
    let compressor_uncompressed = pages_u64(vm.total_uncompressed_pages_in_compressor, page_size);

    Ok(Snapshot {
        hostname: hostname(),
        memory: Memory {
            total,
            available: total.saturating_sub(used),
            cached,
            swap_total: swap.xsu_total,
            swap_free: swap.xsu_total.saturating_sub(swap.xsu_used),
            ..Memory::default()
        },
        details: PlatformDetails::Macos {
            pressure,
            compressor_uncompressed,
            compressor_ram,
        },
        groups: process_groups(prettify, process_count),
    })
}

fn vm_statistics() -> io::Result<VmStatistics64> {
    let mut info = VmStatistics64::default();
    let mut count = HOST_VM_INFO64_REV1_COUNT;
    // SAFETY: `info` matches the rev1 `vm_statistics64` layout (152 bytes / 38
    // integer_t). `host_statistics64` writes at most `count` integer_t values.
    let kr = unsafe {
        host_statistics64(
            mach_host_self(),
            HOST_VM_INFO64,
            (&raw mut info).cast(),
            &raw mut count,
        )
    };
    if kr != KERN_SUCCESS {
        return Err(io::Error::other(format!(
            "host_statistics64 failed with {kr}"
        )));
    }
    Ok(info)
}

fn process_groups(prettify: bool, process_count: usize) -> Vec<ProcessGroup> {
    let mut groups: HashMap<String, (usize, u64)> = HashMap::new();

    for pid in pids() {
        let Some(rss) = process_memory(pid) else {
            continue;
        };
        if rss == 0 {
            continue;
        }
        let Some(executable) = process_name(pid, prettify) else {
            continue;
        };
        let item = groups.entry(executable).or_insert((0, 0));
        item.0 += 1;
        item.1 = item.1.saturating_add(rss);
    }

    rank_process_groups(groups, process_count)
}

fn pids() -> Vec<i32> {
    // SAFETY: A null buffer asks libproc for the required byte size.
    let needed = unsafe { proc_listpids(PROC_ALL_PIDS, 0, ptr::null_mut(), 0) };
    if needed <= 0 {
        return Vec::new();
    }

    let mut buf = vec![0_i32; (needed as usize / mem::size_of::<i32>()).saturating_mul(2) + 16];
    // SAFETY: `buf` is a valid i32 array; libproc writes pid_t values into it.
    let bytes = unsafe {
        proc_listpids(
            PROC_ALL_PIDS,
            0,
            buf.as_mut_ptr().cast(),
            (buf.len() * mem::size_of::<i32>()) as c_int,
        )
    };
    if bytes <= 0 {
        return Vec::new();
    }

    let count = (bytes as usize) / mem::size_of::<i32>();
    buf.truncate(count);
    buf.retain(|&pid| pid > 0);
    buf
}

fn process_memory(pid: i32) -> Option<u64> {
    let mut rusage = MaybeUninit::<RusageInfoV2>::zeroed();
    // SAFETY: `proc_pid_rusage` with RUSAGE_INFO_V2 writes a `rusage_info_v2`
    // (160 bytes) into `rusage`. The C API is declared as `rusage_info_t *`
    // (`void **`) but callers pass a pointer to the struct.
    let rc = unsafe { proc_pid_rusage(pid, RUSAGE_INFO_V2, rusage.as_mut_ptr().cast()) };
    if rc == 0 {
        // SAFETY: the call succeeded, so the struct is initialized.
        let footprint = unsafe { rusage.assume_init().ri_phys_footprint };
        if footprint > 0 {
            return Some(footprint);
        }
    }

    let mut info = MaybeUninit::<ProcTaskInfo>::zeroed();
    // SAFETY: PROC_PIDTASKINFO writes `sizeof(proc_taskinfo)` bytes (96).
    let written = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDTASKINFO,
            0,
            info.as_mut_ptr().cast(),
            mem::size_of::<ProcTaskInfo>() as c_int,
        )
    };
    if written == mem::size_of::<ProcTaskInfo>() as c_int {
        // SAFETY: libproc reported a full proc_taskinfo write.
        let rss = unsafe { info.assume_init().pti_resident_size };
        if rss > 0 {
            return Some(rss);
        }
    }
    None
}

fn process_name(pid: i32, prettify: bool) -> Option<String> {
    let mut path = [0_u8; PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `path` is a writable buffer of PROC_PIDPATHINFO_MAXSIZE bytes.
    let path_len = unsafe { proc_pidpath(pid, path.as_mut_ptr().cast(), PROC_PIDPATHINFO_MAXSIZE) };
    if path_len > 0 {
        let end = path
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(path_len as usize);
        let path = Path::new(std::str::from_utf8(&path[..end]).ok()?);
        let name = path.file_name()?.to_string_lossy();
        if !name.is_empty() {
            return Some(if prettify {
                crate::clean_executable_name(path, &name)
            } else {
                crate::sanitize_name(&name)
            });
        }
    }

    let mut name = [0_u8; 64];
    // SAFETY: `name` is a writable 64-byte buffer; proc_name writes a C string.
    let name_len = unsafe { proc_name(pid, name.as_mut_ptr().cast(), name.len() as u32) };
    if name_len > 0 {
        let end = name
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(name_len as usize);
        let name = std::str::from_utf8(&name[..end]).ok()?;
        if !name.is_empty() {
            return Some(crate::sanitize_name(name));
        }
    }
    None
}

fn page_size() -> u64 {
    let mut size = 0_usize;
    // SAFETY: `host_page_size` writes a single vm_size_t.
    if unsafe { host_page_size(mach_host_self(), &raw mut size) } == KERN_SUCCESS && size > 0 {
        return size as u64;
    }
    sysctl::<i32>("hw.pagesize")
        .ok()
        .filter(|&size| size > 0)
        .map(|size| size as u64)
        .unwrap_or(DEFAULT_PAGE_SIZE)
}

fn hostname() -> String {
    let mut buffer = [0_i8; 256];
    // SAFETY: POSIX gethostname writes a NUL-terminated name into `buffer`.
    if unsafe { gethostname(buffer.as_mut_ptr(), buffer.len()) } == 0 {
        let name = unsafe { CStr::from_ptr(buffer.as_ptr()) };
        if let Ok(name) = name.to_str() {
            let name = name.trim();
            if !name.is_empty() {
                return crate::sanitize_name(name);
            }
        }
    }
    "macos".to_string()
}

fn sysctl<T: Copy + Default>(name: &str) -> io::Result<T> {
    let c_name = CString::new(name).map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut value = T::default();
    let mut len = mem::size_of::<T>();
    // SAFETY: `value` is a T-sized buffer; sysctlbyname writes at most `len` bytes.
    let rc = unsafe {
        sysctlbyname(
            c_name.as_ptr(),
            (&raw mut value).cast(),
            &raw mut len,
            ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(value)
}

fn pages(count: u32, page_size: u64) -> u64 {
    u64::from(count).saturating_mul(page_size)
}

fn pages_u64(count: u64, page_size: u64) -> u64 {
    count.saturating_mul(page_size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rev1_vm_statistics_matches_xnu_layout() {
        assert_eq!(mem::size_of::<VmStatistics64>(), 152);
        assert_eq!(
            mem::size_of::<VmStatistics64>() / mem::size_of::<i32>(),
            HOST_VM_INFO64_REV1_COUNT as usize
        );
        assert_eq!(mem::size_of::<XswUsage>(), 32);
        assert_eq!(mem::size_of::<ProcTaskInfo>(), 96);
        assert_eq!(mem::size_of::<RusageInfoV2>(), 160);
        assert_eq!(mem::offset_of!(RusageInfoV2, ri_phys_footprint), 72);
    }

    #[test]
    fn live_snapshot_has_installed_ram() {
        let snapshot = collect(true, 5).unwrap();
        assert!(snapshot.memory.total >= 512 * 1024 * 1024);
        assert!(snapshot.memory.used() <= snapshot.memory.total);
        assert!(matches!(snapshot.details, PlatformDetails::Macos { .. }));
        assert!(!snapshot.hostname.is_empty());
    }
}
