# ram

`ram` is a small Linux-only memory overview CLI written entirely in Rust. It reads `/proc` and `/sys` directly

## Usage

The display includes total/used/available memory, buffers, cache and reclaimable memory, Linux commit accounting, swap, and zram when exposed by sysfs. “Used” is `MemTotal - MemAvailable`, which is generally more useful for Linux than subtracting only `MemFree`. On older kernels that omit `MemAvailable`, `ram` falls back to a conservative estimate from free, buffer, and reclaimable cache values.

Process rows use resident set size from `/proc/<pid>/statm`. The page size is read directly from `/proc/self/auxv`, with 4096 bytes used only if that interface is unavailable. Rows are grouped by the basename of `/proc/<pid>/exe`, falling back to `comm` and then `cmdline` when permissions or process lifetime make the executable link unavailable. Nix wrapper names such as `.spotify-wrapped` and `clangd-unwrapped` are displayed as `spotify` and `clangd`; use `--no-prettify` to preserve the reported executable name. RSS is summed per executable and the largest ten groups are shown by default; use `-n <COUNT>` to show more entries. Because shared pages can be counted for more than one process, grouped RSS is not a measure of unique physical memory.

Percentages remain numerically truthful when Linux commit accounting or grouped RSS exceeds 100%; the corresponding bars saturate at full width. Styling uses only the terminal's named ANSI palette and default foreground attributes—never fixed RGB values—so it follows the active terminal theme. Styling is enabled only on a terminal and respects `NO_COLOR`; use `--no-color` to force plain output. Watch mode clears the screen only when stdout is a terminal.
