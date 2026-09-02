use std::{
    collections::HashMap,
    env, fs,
    io::{self, IsTerminal, Write},
    path::Path,
    process::ExitCode,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const DEFAULT_PAGE_SIZE: u64 = 4096;
const AT_PAGESZ: usize = 6;
const DEFAULT_PROCESS_COUNT: usize = 10;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Memory {
    total: u64,
    available: u64,
    free: u64,
    buffers: u64,
    cached: u64,
    reclaimable: u64,
    shmem: u64,
    commit_limit: u64,
    committed: u64,
    swap_total: u64,
    swap_free: u64,
}

impl Memory {
    fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    fn cache(&self) -> u64 {
        self.cached
            .saturating_add(self.reclaimable)
            .saturating_sub(self.shmem)
    }

    fn swap_used(&self) -> u64 {
        self.swap_total.saturating_sub(self.swap_free)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessGroup {
    name: String,
    count: usize,
    rss: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Zram {
    configured: u64,
    data: u64,
    compressed: u64,
    memory_used: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct ZramStats {
    data: u64,
    compressed: u64,
    memory_used: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Options {
    no_color: bool,
    no_prettify: bool,
    watch_seconds: Option<u64>,
    process_count: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            no_color: false,
            no_prettify: false,
            watch_seconds: None,
            process_count: DEFAULT_PROCESS_COUNT,
        }
    }
}

enum Command {
    Run(Options),
    Help,
    Version,
}

fn meminfo() -> io::Result<Memory> {
    fs::read_to_string("/proc/meminfo").map(|text| parse_meminfo(&text))
}

fn parse_meminfo(text: &str) -> Memory {
    let mut memory = Memory::default();
    let mut has_available = false;

    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let Some(bytes) = value
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .map(|value| value.saturating_mul(1024))
        else {
            continue;
        };

        match key {
            "MemTotal" => memory.total = bytes,
            "MemAvailable" => {
                memory.available = bytes;
                has_available = true;
            }
            "MemFree" => memory.free = bytes,
            "Buffers" => memory.buffers = bytes,
            "Cached" => memory.cached = bytes,
            "SReclaimable" => memory.reclaimable = bytes,
            "Shmem" => memory.shmem = bytes,
            "CommitLimit" => memory.commit_limit = bytes,
            "Committed_AS" => memory.committed = bytes,
            "SwapTotal" => memory.swap_total = bytes,
            "SwapFree" => memory.swap_free = bytes,
            _ => {}
        }
    }

    if !has_available {
        memory.available = memory
            .free
            .saturating_add(memory.buffers)
            .saturating_add(memory.cache())
            .min(memory.total);
    }

    memory
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

fn parse_single_u64(text: &str) -> Option<u64> {
    text.split_whitespace().next()?.parse().ok()
}

fn parse_zram_mm_stat(text: &str) -> Option<ZramStats> {
    let mut values = text.split_whitespace().take(3).map(str::parse::<u64>);
    Some(ZramStats {
        data: values.next()?.ok()?,
        compressed: values.next()?.ok()?,
        memory_used: values.next()?.ok()?,
    })
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

    let mut groups: Vec<_> = groups
        .into_iter()
        .map(|(name, (count, rss))| ProcessGroup { name, count, rss })
        .collect();
    groups.sort_by(|a, b| b.rss.cmp(&a.rss).then_with(|| a.name.cmp(&b.name)));
    groups.truncate(process_count);
    groups
}

fn parse_statm_rss(text: &str, page_size: u64) -> Option<u64> {
    text.split_whitespace()
        .nth(1)?
        .parse::<u64>()
        .ok()
        .map(|pages| pages.saturating_mul(page_size))
}

fn process_name(dir: &Path, prettify: bool) -> Option<String> {
    if let Ok(target) = fs::read_link(dir.join("exe")) {
        let name = target.file_name().unwrap_or(target.as_os_str());
        let name = name.to_string_lossy();
        let name = name.strip_suffix(" (deleted)").unwrap_or(&name);
        if !name.is_empty() {
            return Some(if prettify {
                clean_executable_name(&target, name)
            } else {
                sanitize_name(name)
            });
        }
    }

    if let Ok(comm) = fs::read(dir.join("comm")) {
        let comm = trim_ascii_whitespace(&comm);
        if !comm.is_empty() {
            return Some(sanitize_name(&String::from_utf8_lossy(comm)));
        }
    }

    let command_line = fs::read(dir.join("cmdline")).ok()?;
    let first = command_line.split(|byte| *byte == 0).next()?;
    let name = first.rsplit(|byte| *byte == b'/').next()?;
    if name.is_empty() {
        None
    } else {
        Some(sanitize_name(&String::from_utf8_lossy(name)))
    }
}

fn clean_executable_name(path: &Path, name: &str) -> String {
    let cleaned = if path.starts_with("/nix/store") {
        let wrapped = name
            .strip_prefix('.')
            .and_then(|name| name.strip_suffix("-wrapped"))
            .filter(|name| !name.is_empty());
        let unwrapped = name
            .strip_suffix("-unwrapped")
            .map(|name| name.strip_prefix('.').unwrap_or(name))
            .filter(|name| !name.is_empty());
        wrapped.or(unwrapped).unwrap_or(name)
    } else {
        name
    };
    sanitize_name(cleaned)
}

fn trim_ascii_whitespace(mut bytes: &[u8]) -> &[u8] {
    while bytes.first().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[1..];
    }
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes = &bytes[..bytes.len() - 1];
    }
    bytes
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_control() {
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect()
}

fn page_size() -> u64 {
    fs::read("/proc/self/auxv")
        .ok()
        .and_then(|bytes| parse_auxv_page_size(&bytes))
        .unwrap_or(DEFAULT_PAGE_SIZE)
}

fn parse_auxv_page_size(bytes: &[u8]) -> Option<u64> {
    let word_size = std::mem::size_of::<usize>();
    for entry in bytes.chunks_exact(word_size * 2) {
        let key = usize::from_ne_bytes(entry[..word_size].try_into().ok()?);
        let value = usize::from_ne_bytes(entry[word_size..].try_into().ok()?);
        if key == AT_PAGESZ && value > 0 {
            return u64::try_from(value).ok();
        }
        if key == 0 {
            break;
        }
    }
    None
}

fn format_bytes(bytes: u64) -> String {
    let units = ["B", "K", "M", "G", "T", "P", "E"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 || unit == 0 {
        format!("{value:.0} {}", units[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", units[unit])
    } else {
        format!("{value:.2} {}", units[unit])
    }
}

fn render(
    out: &mut impl Write,
    color: bool,
    prettify: bool,
    process_count: usize,
) -> io::Result<()> {
    let memory = meminfo()?;
    let zram = zram_info();
    let groups = process_groups(prettify, process_count);
    render_snapshot(out, color, process_count, &memory, &zram, &groups)
}

fn render_snapshot(
    out: &mut impl Write,
    color: bool,
    process_count: usize,
    memory: &Memory,
    zram: &Zram,
    groups: &[ProcessGroup],
) -> io::Result<()> {
    // Standard ANSI palette roles are resolved by the active terminal theme.
    let (accent, positive, dim, bold, reset) = if color {
        ("\x1b[36m", "\x1b[32m", "\x1b[2m", "\x1b[1m", "\x1b[0m")
    } else {
        ("", "", "", "", "")
    };

    let hostname = hostname();
    writeln!(
        out,
        "{accent}{bold}RAM USAGE{reset}  {dim}—{reset}  {bold}{hostname}{reset}  {dim}{}{reset}",
        utc_clock()
    )?;
    writeln!(out, "{dim}{}{reset}", "─".repeat(78))?;
    writeln!(
        out,
        "[{}]  {:>5}",
        styled_bar(memory.used(), memory.total, 67, positive, dim, reset),
        format_percent(memory.used(), memory.total)
    )?;
    writeln!(out)?;
    writeln!(
        out,
        "  Used      {bold}{:>9}{reset}    Available  {bold}{:>9}{reset}    Total  {bold}{:>9}{reset}",
        format_bytes(memory.used()),
        format_bytes(memory.available),
        format_bytes(memory.total)
    )?;
    writeln!(
        out,
        "  Commit    {bold}{:>9}{reset} / {bold}{:>9}{reset}   {dim}({} of commit limit){reset}",
        format_bytes(memory.committed),
        format_bytes(memory.commit_limit),
        format_percent(memory.committed, memory.commit_limit)
    )?;
    writeln!(
        out,
        "  Cached    {bold}{:>9}{reset}               {dim}(reclaimable on demand){reset}",
        format_bytes(memory.cache())
    )?;
    let swap_note = if zram.configured > 0 {
        "zram may be compressed; CPU, not disk"
    } else {
        "disk-backed swap"
    };
    writeln!(
        out,
        "  Swap      {bold}{:>9}{reset} / {bold}{:>9}{reset}   {dim}({swap_note}){reset}",
        format_bytes(memory.swap_used()),
        format_bytes(memory.swap_total)
    )?;
    if zram.data > 0 || zram.memory_used > 0 {
        writeln!(
            out,
            "  Zram      {bold}{:>9}{reset} data, {bold}{:>9}{reset} compressed, {bold}{:>9}{reset} RAM",
            format_bytes(zram.data),
            format_bytes(zram.compressed),
            format_bytes(zram.memory_used)
        )?;
    }

    writeln!(
        out,
        "\n{accent}{bold}TOP {process_count} PROCESSES BY RESIDENT SET{reset}"
    )?;
    writeln!(out, "{dim}{}{reset}", "─".repeat(78))?;

    let largest_rss = groups.first().map_or(0, |process| process.rss);
    for process in groups {
        let label = if process.count > 1 {
            format!("{} ({})", process.name, process.count)
        } else {
            process.name.clone()
        };
        writeln!(
            out,
            "{} {bold}{:>9}{reset}  {}  {dim}{:>5}{reset}",
            fit_width(&label, 27),
            format_bytes(process.rss),
            styled_bar(process.rss, largest_rss, 27, positive, dim, reset),
            format_percent(process.rss, memory.total)
        )?;
    }
    let top_rss = groups
        .iter()
        .fold(0_u64, |total, process| total.saturating_add(process.rss));
    writeln!(out, "{dim}{}{reset}", "─".repeat(78))?;
    writeln!(
        out,
        "{dim}These {} account for{reset}  {bold}{:>9}{reset}  {dim}({} of installed RAM){reset}",
        groups.len(),
        format_bytes(top_rss),
        format_percent(top_rss, memory.total)
    )?;
    Ok(())
}

fn styled_bar(
    value: u64,
    total: u64,
    width: usize,
    positive: &str,
    dim: &str,
    reset: &str,
) -> String {
    let filled = if total == 0 {
        0
    } else {
        (u128::from(value) * width as u128 / u128::from(total)) as usize
    }
    .min(width);
    format!(
        "{positive}{}{dim}{}{reset}",
        "█".repeat(filled),
        "░".repeat(width - filled)
    )
}

fn format_percent(value: u64, total: u64) -> String {
    if total == 0 {
        return "0.0%".to_string();
    }
    let tenths = u128::from(value) * 1000 / u128::from(total);
    format!("{}.{:01}%", tenths / 10, tenths % 10)
}

fn hostname() -> String {
    fs::read("/proc/sys/kernel/hostname")
        .ok()
        .map(|bytes| trim_ascii_whitespace(&bytes).to_vec())
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| sanitize_name(&String::from_utf8_lossy(&bytes)))
        .unwrap_or_else(|| "linux".to_string())
}

fn utc_clock() -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = seconds / 86_400;
    let clock = seconds % 86_400;
    let weekday = WEEKDAYS[((days + 4) % 7) as usize];
    format!(
        "{weekday} {:02}:{:02}:{:02} UTC",
        clock / 3600,
        (clock / 60) % 60,
        clock % 60
    )
}

fn fit_width(text: &str, width: usize) -> String {
    let current_width = display_width(text);
    if current_width <= width {
        return format!("{text}{}", " ".repeat(width - current_width));
    }
    if width == 0 {
        return String::new();
    }

    let target = width - 1;
    let mut result = String::new();
    let mut used = 0;
    for character in text.chars() {
        let character_width = char_width(character);
        if used + character_width > target {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result.push('…');
    result.push_str(&" ".repeat(width - used - 1));
    result
}

fn display_width(text: &str) -> usize {
    text.chars().map(char_width).sum()
}

fn char_width(character: char) -> usize {
    let code = character as u32;
    if character.is_control()
        || (0x0300..=0x036f).contains(&code)
        || (0x1ab0..=0x1aff).contains(&code)
        || (0x1dc0..=0x1dff).contains(&code)
        || (0x20d0..=0x20ff).contains(&code)
        || (0xfe00..=0xfe0f).contains(&code)
        || (0xfe20..=0xfe2f).contains(&code)
        || (0xe0100..=0xe01ef).contains(&code)
    {
        0
    } else if (0x1100..=0x115f).contains(&code)
        || (0x2329..=0x232a).contains(&code)
        || (0x2e80..=0xa4cf).contains(&code)
        || (0xac00..=0xd7a3).contains(&code)
        || (0xf900..=0xfaff).contains(&code)
        || (0xfe10..=0xfe19).contains(&code)
        || (0xfe30..=0xfe6f).contains(&code)
        || (0xff00..=0xff60).contains(&code)
        || (0xffe0..=0xffe6).contains(&code)
        || (0x1f300..=0x1faff).contains(&code)
        || (0x20000..=0x3fffd).contains(&code)
    {
        2
    } else {
        1
    }
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command, String> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "-V" | "--version" => return Ok(Command::Version),
            "--no-color" => options.no_color = true,
            "--no-prettify" => options.no_prettify = true,
            "-n" => {
                let value = args
                    .next()
                    .ok_or_else(|| "-n requires a number of entries".to_string())?;
                options.process_count = parse_count(&value)?;
            }
            "--watch" | "--interval" => {
                let value = args
                    .next()
                    .ok_or_else(|| format!("{argument} requires a number of seconds"))?;
                options.watch_seconds = Some(parse_interval(&value)?);
            }
            _ if argument.starts_with("--watch=") => {
                options.watch_seconds = Some(parse_interval(&argument[8..])?);
            }
            _ if argument.starts_with("--interval=") => {
                options.watch_seconds = Some(parse_interval(&argument[11..])?);
            }
            _ => return Err(format!("unknown option: {argument}")),
        }
    }
    Ok(Command::Run(options))
}

fn parse_interval(value: &str) -> Result<u64, String> {
    match value.parse::<u64>() {
        Ok(0) | Err(_) => Err(format!(
            "invalid interval: {value:?} (expected a positive integer)"
        )),
        Ok(seconds) => Ok(seconds),
    }
}

fn parse_count(value: &str) -> Result<usize, String> {
    match value.parse::<usize>() {
        Ok(0) | Err(_) => Err(format!(
            "invalid entry count: {value:?} (expected a positive integer)"
        )),
        Ok(count) => Ok(count),
    }
}

fn print_help(out: &mut impl Write) -> io::Result<()> {
    writeln!(
        out,
        "ram {}\n\nLinux memory overview\n\nUSAGE:\n    ram [OPTIONS]\n\nOPTIONS:\n    -n <COUNT>            Show this many process entries [default: 10]\n    --no-color            Disable ANSI colors\n    --no-prettify         Keep executable names exactly as reported\n    --watch <SECONDS>     Refresh repeatedly\n    --interval <SECONDS>  Alias for --watch\n    -h, --help            Print help\n    -V, --version         Print version",
        env!("CARGO_PKG_VERSION")
    )
}

fn run() -> io::Result<()> {
    let command = parse_args(env::args().skip(1))
        .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
    let mut stdout = io::stdout().lock();

    let options = match command {
        Command::Help => return print_help(&mut stdout),
        Command::Version => {
            return writeln!(stdout, "ram {}", env!("CARGO_PKG_VERSION"));
        }
        Command::Run(options) => options,
    };

    let terminal = io::stdout().is_terminal();
    let color = !options.no_color
        && terminal
        && env::var("NO_COLOR").is_err()
        && env::var("TERM").is_ok_and(|value| value != "dumb");

    loop {
        if options.watch_seconds.is_some() && terminal {
            write!(stdout, "\x1b[2J\x1b[H")?;
        }
        render(
            &mut stdout,
            color,
            !options.no_prettify,
            options.process_count,
        )?;
        stdout.flush()?;
        if let Some(seconds) = options.watch_seconds {
            thread::sleep(Duration::from_secs(seconds));
        } else {
            break;
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => ExitCode::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
            eprintln!("ram: {error}\nTry 'ram --help' for more information.");
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("ram: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meminfo_and_derives_metrics() {
        let memory = parse_meminfo(
            "MemTotal:       1000 kB\n\
             MemFree:         100 kB\n\
             MemAvailable:    600 kB\n\
             Buffers:          20 kB\n\
             Cached:          300 kB\n\
             SReclaimable:     50 kB\n\
             Shmem:            25 kB\n\
             CommitLimit:    2000 kB\n\
             Committed_AS:   2100 kB\n\
             SwapTotal:       500 kB\n\
             SwapFree:        125 kB\n",
        );

        assert_eq!(memory.total, 1000 * 1024);
        assert_eq!(memory.used(), 400 * 1024);
        assert_eq!(memory.cache(), 325 * 1024);
        assert_eq!(memory.swap_used(), 375 * 1024);
    }

    #[test]
    fn estimates_available_memory_when_field_is_missing() {
        let memory = parse_meminfo(
            "MemTotal: 1000 kB\nMemFree: 100 kB\nBuffers: 50 kB\nCached: 200 kB\nSReclaimable: 25 kB\nShmem: 10 kB\n",
        );
        assert_eq!(memory.available, 365 * 1024);
        assert_eq!(memory.used(), 635 * 1024);
    }

    #[test]
    fn parses_zram_and_statm_fixtures() {
        assert_eq!(
            parse_zram_mm_stat("1048576 262144 327680 0 0 0 0 0"),
            Some(ZramStats {
                data: 1_048_576,
                compressed: 262_144,
                memory_used: 327_680,
            })
        );
        assert_eq!(parse_zram_mm_stat("bad data"), None);
        assert_eq!(parse_statm_rss("100 25 10 0 0 0 0", 16_384), Some(409_600));
        assert_eq!(parse_statm_rss("100", 4096), None);
    }

    #[test]
    fn parses_native_auxv_page_size() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&AT_PAGESZ.to_ne_bytes());
        bytes.extend_from_slice(&16_384usize.to_ne_bytes());
        bytes.extend_from_slice(&0usize.to_ne_bytes());
        bytes.extend_from_slice(&0usize.to_ne_bytes());
        assert_eq!(parse_auxv_page_size(&bytes), Some(16_384));
    }

    #[test]
    fn percentages_can_exceed_one_hundred_without_overflow() {
        assert_eq!(format_percent(5, 10), "50.0%");
        assert_eq!(format_percent(1, 0), "0.0%");
        assert_eq!(format_percent(21, 10), "210.0%");
        assert_eq!(format_percent(u64::MAX, u64::MAX), "100.0%");
    }

    #[test]
    fn bars_have_stable_width_and_saturate() {
        assert_eq!(styled_bar(1, 2, 10, "", "", "").chars().count(), 10);
        assert_eq!(styled_bar(0, 0, 4, "", "", ""), "░░░░");
        assert_eq!(styled_bar(20, 10, 4, "", "", ""), "████");
    }

    #[test]
    fn bytes_are_human_readable() {
        assert_eq!(format_bytes(1024), "1.00 K");
        assert_eq!(format_bytes(1024 * 1024), "1.00 M");
        assert_eq!(format_bytes(u64::MAX), "16.0 E");
    }

    #[test]
    fn snapshot_matches_the_compact_dashboard_structure() {
        let memory = Memory {
            total: 8 * 1024 * 1024 * 1024,
            available: 6 * 1024 * 1024 * 1024,
            committed: 6 * 1024 * 1024 * 1024,
            commit_limit: 10 * 1024 * 1024 * 1024,
            ..Memory::default()
        };
        let groups = vec![ProcessGroup {
            name: "browser".to_string(),
            count: 3,
            rss: 1024 * 1024 * 1024,
        }];
        let mut output = Vec::new();
        render_snapshot(
            &mut output,
            false,
            DEFAULT_PROCESS_COUNT,
            &memory,
            &Zram::default(),
            &groups,
        )
        .unwrap();
        let output = String::from_utf8(output).unwrap();

        assert!(output.contains("RAM USAGE"));
        assert!(output.contains("Used"));
        assert!(output.contains("Available"));
        assert!(output.contains("TOP 10 PROCESSES BY RESIDENT SET"));
        assert!(output.contains("browser (3)"));
        assert!(output.contains("These 1 account for"));
        assert!(!output.contains("\x1b["));
    }

    #[test]
    fn process_names_are_safe_and_unicode_width_is_stable() {
        assert_eq!(sanitize_name("evil\x1b[31m"), "evil�[31m");
        assert_eq!(display_width("memory"), 6);
        assert_eq!(display_width("内存"), 4);
        assert_eq!(display_width("e\u{301}"), 1);
        assert_eq!(display_width(&fit_width("very-long-process-name", 10)), 10);
        assert_eq!(display_width(&fit_width("内存监控程序", 8)), 8);
    }

    #[test]
    fn cleans_nix_wrapped_executable_names_only() {
        assert_eq!(
            clean_executable_name(
                Path::new("/nix/store/hash-spotify/bin/.spotify-wrapped"),
                ".spotify-wrapped"
            ),
            "spotify"
        );
        assert_eq!(
            clean_executable_name(
                Path::new("/nix/store/hash-zed/bin/.zed-editor-wrapped"),
                ".zed-editor-wrapped"
            ),
            "zed-editor"
        );
        assert_eq!(
            clean_executable_name(
                Path::new("/nix/store/hash-clang/bin/clangd-unwrapped"),
                "clangd-unwrapped"
            ),
            "clangd"
        );
        assert_eq!(
            clean_executable_name(Path::new("/opt/app/.spotify-wrapped"), ".spotify-wrapped"),
            ".spotify-wrapped"
        );
        assert_eq!(
            clean_executable_name(Path::new("/opt/app/clangd-unwrapped"), "clangd-unwrapped"),
            "clangd-unwrapped"
        );
        assert_eq!(
            clean_executable_name(Path::new("/nix/store/hash-app/bin/ordinary"), "ordinary"),
            "ordinary"
        );
    }

    #[test]
    fn parses_cli_options_and_rejects_bad_intervals() {
        let Command::Run(options) =
            parse_args(["--interval=2".to_string(), "--no-prettify".to_string()]).unwrap()
        else {
            panic!("expected run command");
        };
        assert_eq!(options.watch_seconds, Some(2));
        assert_eq!(options.process_count, DEFAULT_PROCESS_COUNT);
        assert!(options.no_prettify);
        let Command::Run(options) = parse_args(["-n".to_string(), "25".to_string()]).unwrap()
        else {
            panic!("expected run command");
        };
        assert_eq!(options.process_count, 25);
        assert!(parse_args(["-n".to_string(), "0".to_string()]).is_err());
        assert!(parse_args(["-n".to_string()]).is_err());
        assert!(parse_args(["--watch".to_string(), "0".to_string()]).is_err());
        assert!(parse_args(["--unknown".to_string()]).is_err());
    }
}
