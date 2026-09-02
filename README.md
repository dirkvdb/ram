# ram

`ram` is a small memory overview CLI written entirely in Rust.

- **Linux** reads `/proc` and `/sys` directly.
- **macOS** reads Mach (`host_statistics64`), `sysctl`, and `libproc`. Process
  sizes use physical footprint (Activity Monitor's Memory column). There is no
  Linux-style commit limit, so the dashboard shows memory pressure and the
  in-RAM compressor instead of commit/zram.

### Usage

```text
$ ram --help
ram 0.1.0

Linux and macOS memory overview

USAGE:
    ram [OPTIONS]

OPTIONS:
    -n <COUNT>            Show this many process entries [default: 10]
    --no-color            Disable ANSI colors
    --no-prettify         Keep executable names exactly as reported
    --watch <SECONDS>     Refresh repeatedly
    --interval <SECONDS>  Alias for --watch
    -h, --help            Print help
    -V, --version         Print version
```

### Example output (Linux)

```text
RAM USAGE  —  p260182  Wed 14:28:30 UTC
──────────────────────────────────────────────────────────────────────────────
[███████████████░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░]  23.1%

  Used         14.4 G    Available     47.8 G    Total     62.3 G
  Commit       31.9 G /    39.9 G   (79.9% of commit limit)
  Cached       26.2 G               (reclaimable on demand)
  Swap            0 B /    8.78 G   (disk-backed swap)

TOP 10 PROCESSES BY RESIDENT SET
──────────────────────────────────────────────────────────────────────────────
zen (26)                       7.29 G  ███████████████████████████  11.7%
ferdium (11)                   2.18 G  ████████░░░░░░░░░░░░░░░░░░░   3.4%
electron (10)                  1.83 G  ██████░░░░░░░░░░░░░░░░░░░░░   2.9%
slack (8)                      1.14 G  ████░░░░░░░░░░░░░░░░░░░░░░░   1.8%
editors_helper (7)             1.09 G  ████░░░░░░░░░░░░░░░░░░░░░░░   1.7%
ghostty (2)                     364 M  █░░░░░░░░░░░░░░░░░░░░░░░░░░   0.5%
gitcomet                        350 M  █░░░░░░░░░░░░░░░░░░░░░░░░░░   0.5%
.Hyprland-wrapp                 285 M  █░░░░░░░░░░░░░░░░░░░░░░░░░░   0.4%
DesktopEditors                  266 M  ░░░░░░░░░░░░░░░░░░░░░░░░░░░   0.4%
fastpotify                      239 M  ░░░░░░░░░░░░░░░░░░░░░░░░░░░   0.3%
──────────────────────────────────────────────────────────────────────────────
These 10 account for     15.0 G  (24.0% of installed RAM)
```

### Example output (macOS)

```text
RAM USAGE  —  macbook-robin.local  Wed 14:52:02 UTC
──────────────────────────────────────────────────────────────────────────────
[██████████████████████████████████████████████████████░░░░░░░░░░░░░]  81.6%

  Used         29.4 G    Available     6.59 G    Total     36.0 G
  Pressure     normal               (macOS memory pressure)
  Cached       5.05 G               (file-backed, reclaimable)
  Swap            0 B /       0 B   (disk-backed swap)
  Compress     23.5 G stored,    11.1 G in RAM   (in-memory compressor)

TOP 10 PROCESSES BY MEMORY
──────────────────────────────────────────────────────────────────────────────
qemu-system-aarch64            3.63 G  ███████████████████████████  10.0%
Cursor Helper (Renderer) (…    2.67 G  ███████████████████░░░░░░░░   7.4%
Browser Helper (Renderer) …    2.59 G  ███████████████████░░░░░░░░   7.1%
Cursor Helper (Plugin) (24)    2.32 G  █████████████████░░░░░░░░░░   6.4%
Cursor Helper (13)             1.90 G  ██████████████░░░░░░░░░░░░░   5.2%
MTLCompilerService (64)        1.09 G  ████████░░░░░░░░░░░░░░░░░░░   3.0%
Microsoft Teams WebView He…    1.07 G  ███████░░░░░░░░░░░░░░░░░░░░   2.9%
Arc                            1.00 G  ███████░░░░░░░░░░░░░░░░░░░░   2.7%
Slack Helper (5)                880 M  ██████░░░░░░░░░░░░░░░░░░░░░   2.3%
Codex (Service) (5)             783 M  █████░░░░░░░░░░░░░░░░░░░░░░   2.1%
──────────────────────────────────────────────────────────────────────────────
These 10 account for     17.9 G  (49.7% of installed RAM)
```
