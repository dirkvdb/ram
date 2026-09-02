# ram

`ram` is a small Linux-only memory overview CLI written entirely in Rust. It reads `/proc` and `/sys` directly

### Usage

```text
$ ram --help
ram 0.1.0

Linux memory overview

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

### Example output

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
