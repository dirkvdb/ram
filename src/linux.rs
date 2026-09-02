use std::{collections::HashMap, fs, io, path::Path};

use crate::{
    DEFAULT_PAGE_SIZE, Memory, PlatformDetails, ProcessGroup, Snapshot, Zram, parse_auxv_page_size,
    parse_single_u64, parse_statm_rss, parse_zram_mm_stat, rank_process_groups,
};

pub(crate) fn collect(prettify: bool, process_count: usize) -> io::Result<Snapshot> {
    Ok(Snapshot {
        hostname: hostname(),
        memory: meminfo()?,
        details: PlatformDetails::Linux { zram: zram_info() },
        groups: process_groups(prettify, process_count),
    })
}

fn meminfo() -> io::Result<Memory> {
    fs::read_to_string("/proc/meminfo").map(|text| crate::parse_meminfo(&text))
}

fn zram_info() -> Zram {
    let mut result = Zram::default();
    let Ok(entries) = fs::read_dir("/sys/block") else {
        return result;
    };

    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().starts_with("zram") {
            continue;
        }

        let dir = entry.path();
        if let Ok(text) = fs::read_to_string(dir.join("disksize")) {
            result.configured = result
                .configured
                .saturating_add(parse_single_u64(&text).unwrap_or(0));
        }
        if let Ok(text) = fs::read_to_string(dir.join("mm_stat")) {
            if let Some(stats) = parse_zram_mm_stat(&text) {
                result.data = result.data.saturating_add(stats.data);
                result.compressed = result.compressed.saturating_add(stats.compressed);
                result.memory_used = result.memory_used.saturating_add(stats.memory_used);
            }
        }
    }

    result
}

fn process_groups(prettify: bool, process_count: usize) -> Vec<ProcessGroup> {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Vec::new();
    };
    let page_size = page_size();
    let mut groups: HashMap<String, (usize, u64)> = HashMap::new();

    for entry in entries.flatten() {
        let filename = entry.file_name();
        if !filename.as_encoded_bytes().iter().all(u8::is_ascii_digit) {
            continue;
        }

        let dir = entry.path();
        let Some(rss) = fs::read_to_string(dir.join("statm"))
            .ok()
            .and_then(|text| parse_statm_rss(&text, page_size))
        else {
            continue;
        };
        if rss == 0 {
            continue;
        }

        let Some(executable) = process_name(&dir, prettify) else {
            continue;
        };
        let item = groups.entry(executable).or_insert((0, 0));
        item.0 += 1;
        item.1 = item.1.saturating_add(rss);
    }

    rank_process_groups(groups, process_count)
}

fn process_name(dir: &Path, prettify: bool) -> Option<String> {
    if let Ok(target) = fs::read_link(dir.join("exe")) {
        let name = target.file_name().unwrap_or(target.as_os_str());
        let name = name.to_string_lossy();
        let name = name.strip_suffix(" (deleted)").unwrap_or(&name);
        if !name.is_empty() {
            return Some(if prettify {
                crate::clean_executable_name(&target, name)
            } else {
                crate::sanitize_name(name)
            });
        }
    }

    if let Ok(comm) = fs::read(dir.join("comm")) {
        let comm = crate::trim_ascii_whitespace(&comm);
        if !comm.is_empty() {
            return Some(crate::sanitize_name(&String::from_utf8_lossy(comm)));
        }
    }

    let command_line = fs::read(dir.join("cmdline")).ok()?;
    let first = command_line.split(|byte| *byte == 0).next()?;
    let name = first.rsplit(|byte| *byte == b'/').next()?;
    if name.is_empty() {
        None
    } else {
        Some(crate::sanitize_name(&String::from_utf8_lossy(name)))
    }
}

fn page_size() -> u64 {
    fs::read("/proc/self/auxv")
        .ok()
        .and_then(|bytes| parse_auxv_page_size(&bytes))
        .unwrap_or(DEFAULT_PAGE_SIZE)
}

fn hostname() -> String {
    fs::read("/proc/sys/kernel/hostname")
        .ok()
        .map(|bytes| crate::trim_ascii_whitespace(&bytes).to_vec())
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| crate::sanitize_name(&String::from_utf8_lossy(&bytes)))
        .unwrap_or_else(|| "linux".to_string())
}
