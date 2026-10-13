# Performance profiling

Zetta provides an on-screen performance overlay, reproducible terminal-rendering
workloads, machine-readable reports, and platform diagnostics.

Always use an optimized build when recording or comparing measurements.

## Output throughput benchmark

Use the output benchmark to compare how quickly terminal emulators consume the
same text stream:

```sh
cargo build --release
target/release/zetta benchmark output
```

The command writes 10 MiB of repeated lines by default. Set another output
size in MiB with `--size` or `-s`:

```sh
target/release/zetta benchmark output --size 100
```

Use `--output-type unique` (or `-t unique`) to write deterministic lines that never repeat.
This is the worst-case workload for scrollback history archival, where repeated
lines may otherwise be compacted efficiently:

```sh
target/release/zetta benchmark output --output-type unique --size 100
```

The deterministic, printable ASCII text is written to standard output in
128 KiB blocks, flushed, and then the elapsed time and throughput are printed
to standard error. The result identifies the output type and detected terminal
size in columns × rows. Payload construction is excluded from the measurement,
and the measured standard-output stream contains no timing metadata.

Run the same optimized binary and command inside each terminal emulator. The
result measures the time for the process to write and flush the payload,
including terminal or PTY backpressure, like timing `cat` on an equivalently
sized text file. It does not measure when the terminal finishes presenting the
last frame on the GPU. Avoid redirecting standard output when comparing
terminal emulators, because that measures the redirected destination instead.

For a scrollback-scaling check, run the benchmark repeatedly in the same pane:

```sh
for run in 1 2 3 4 5 6 7 8 9 10; do
  target/release/zetta benchmark output
done
```

Compare both the individual results and their trend. A fresh pane provides the
cold-history baseline; repeated runs reveal ingestion or rendering work that
grows with retained scrollback. Window size materially affects throughput,
especially for unique lines and history archival, so compare only results that
report the same terminal columns and rows. Use the equivalent loop syntax in
PowerShell or Command Prompt on Windows.

### Windows ConPTY throughput

The native Windows comparison in Shitty's `dev/compare_windows.py` pipes `cat`
through a byte relay that calls `WriteFile` on console stdout. ConPTY interprets
that output before Zetta receives UTF-8. This differs from Unix, where `cat`
delivers the original random bytes to the terminal's parser.

On the Windows test host, both Zetta and Windows Terminal inherited console code
page 850. The 100,000,000-byte random payload became 180,984,122 bytes at Zetta's
reader, with code-page characters and generated terminal sequences. The
standalone parser tests, which bypass ConPTY, measured about 412 MiB/s for ASCII
and 80 MiB/s for raw random bytes on an 80x24 grid with 500 history lines. A
separate UTF-8 console-mode experiment still measured about 14 MiB/s through
ConPTY. Console conversion and its own terminal processing therefore remain
part of the native random result; changing the code page does not turn it into
the Unix parser workload.

The Windows source reader previously waited up to 2 ms for its entire 1 MiB
read-ahead pipe to fill before each read, even though the event loop requested
at most 65,535 bytes. Publishing available bytes immediately removes that wait
and the per-source-read refill mutex and condition-variable notification. The
output kernel pipe is now 128 KiB; input capacity and the bounded user-space
read-ahead are unchanged.

Native-parser wakeups are also coalesced at the sender, before they schedule
foreground work. Coalescing only after delivery still woke the Windows UI
thread for each parser batch. The UI clears the pending flag before inspecting
output, so a concurrent parser update can request another redraw. Other
terminal events retain their delivery order.

The 2026-10-12 graphical comparison used optimized builds, an 80x24 grid,
500 history lines, Consolas 12 pt, and matched 11x23 backing-pixel cells at
120 DPI. Three runs of each payload were interleaved across the original
Zetta binary, the updated binary, and Windows Terminal; no build ran alongside
them. Best wall times include process startup and shutdown:

| Payload | Original Zetta | Updated Zetta | Windows Terminal |
| --- | --- | --- | --- |
| ASCII, 1,000,000,000 bytes | 7.325 s / 130.2 MiB/s | 6.390 s / 149.3 MiB/s | 5.668 s / 168.3 MiB/s |
| Random, 100,000,000 bytes | 6.978 s / 13.7 MiB/s | 6.699 s / 14.2 MiB/s | 9.237 s / 10.3 MiB/s |

This improves ASCII throughput by 15% and random throughput by 4%. Windows
Terminal still leads ASCII by 13%; these changes narrow that gap. The complete
run data is retained in `artifacts/windows-throughput/final-comparison.json`,
with the console conversion probe, standalone parser measurements, and native
ConPTY diagnostics beside it. Compare paired results on the same host rather
than treating throughput from a different session as the baseline.

Three paired ten-second runs of the standard graphical workload retained
roughly 60 FPS. Median mean draw time was 1.395 ms before and 1.377 ms after.
Median process CPU time was 2.047 s before and 2.156 s after, an increase of
about 1.1% of one core across the ten-second run. The individual reports are
`artifacts/windows-throughput/render-{before,after}-{1,2,3}.json`.

A child exit also needs to drain more than one gather batch. Windows keeps
draining queued output and allows a 20 ms quiet period for the source thread
to publish its final bytes before joining the parser. A 4 MiB bound prevents a
surviving descendant from extending the drain indefinitely. The regression
test waits for the real child to exit while the grid is locked; a fixed delay
could otherwise expire during PowerShell startup and miss the output-loss case.

Run the real ConPTY benchmarks from the terminal engine's workspace, with no
other builds or benchmarks competing for CPU:

```sh
cd crates/alacritty_terminal
cargo test --release conpty_throughput_benchmark -- --ignored --nocapture --test-threads=1
```

These include producer startup and ConPTY processing, and report parsing time
separately from wall time. The `ascii_output_throughput_benchmark` and
`binary_output_throughput_benchmark` filters measure parsing without ConPTY.
Use the full graphical comparison to assess application throughput; neither
standalone measurement includes GPUI rendering.

### macOS PTY throughput

macOS hands the PTY master roughly 1 KiB per read even with a larger buffer.
Returning to kqueue at every brief producer refill gap put the reader and parser
to sleep repeatedly. The ASCII parser initially measured 512 MiB/s alone, but
only 146 MiB/s through a real PTY.

The macOS reader gathers into four 256 KiB buffers, retaining the existing 1 MiB
read-ahead bound. The parser consumes each buffer in at most 65,535-byte segments,
releasing the terminal lock between them. A buffer returns to the reader only
once its final segment has been parsed. One-shot kqueue readiness removes the
filter while gathering and is rearmed after every poll.

Small bursts retry up to sixteen nonblocking reads at a refill gap. Consecutive
chunks carrying at least 8 KiB collectively get up to 256 retries, until a 1 ms
quiet gap resets that classification. This preserves bulk throughput without
spinning through the larger budget after every small rendering-workload burst.
A three-millisecond budget spans all refill gaps in a batch, checked every
sixteen failed reads; successes never restart it. Empty batches, EOF and errors
end immediately. Other platforms retain their original gather size and polling
policy.

Unix PTY hangups can race with replies generated by escape sequences in random
output. A master write returning `EIO` now waits for the child's exit report,
preserving the final drain, rather than opening a backend-error pane. A regression
test forces the write failure before the real child notification and fails when
that handling is removed. Only nonempty output clears a pending foreign-child
hangup deadline; EOF cannot restart its bounded wait for a missing exit report.

SIMD escape searches, a portable word-at-a-time ASCII boundary scan, and writing
only characters when a blank row already has the right attributes reduce parser
work too. The final parse-only test measured about 760 MiB/s and the isolated
PTY pipeline about 297 MiB/s.

Apple Silicon, macOS 26.6.2, Rust 1.99.0, recorded 2026-10-11, optimized builds:
Ghostty 1.3.2-dev+b1d9492cfa5d in ReleaseFast mode.

| GUI payload | Initial Zetta | First retry optimization | Final Zetta | Ghostty |
| --- | --- | --- | --- | --- |
| 1,000,000,000 repeated ASCII bytes | 7.02 s / 136 MiB/s | 4.84 s / 197 MiB/s | 3.80 s / 251 MiB/s | 3.99 s / 239 MiB/s |
| 100,000,000 seeded random bytes | 1.22 s / 78 MiB/s | 1.19 s / 80 MiB/s | 1.18 s / 81 MiB/s | 3.57 s / 27 MiB/s |

Best wall times of three warm launches, with no concurrent compilation: 80×24,
Menlo 12, 14×28 backing-pixel cells, 500 history lines, `--no-mux`. The final
comparison uses Shitty's `dev/compare.py`, retaining each launch's stderr and
verifying matching grids and cells. A repeat measured Zetta at 252 MiB/s versus
Ghostty at 241 MiB/s for ASCII, and 79 versus 27 MiB/s for random. Twenty more
consecutive random-data runs exited cleanly, with no backend errors. These
timings measure ingestion and process exit, rather than presentation of the
last frame.

The ten-second standard 240 Hz rendering workload drew 602 frames, approximately
60 draw FPS. Average CPU draw time was 2.78 ms versus 2.69 ms for the initial
build; p95 was 4.30 ms versus 4.36 ms. Process CPU usage averaged 32.2% of one
core versus 31.6%. Small-burst classification reduced the 38% measured with
the unconditional bulk retry budget. Raw timing records, native samples,
rendering JSON reports, and validation logs are kept under
`artifacts/macos-throughput/`; the final runs are under `round2/`.

The ignored `ascii_through_the_event_loop_throughput_benchmark` and
`random_through_the_event_loop_throughput_benchmark` tests in
`crates/alacritty_terminal` isolate the PTY pipeline. Run them with
`cargo test --release through_the_event_loop_throughput_benchmark -- --ignored --nocapture`
from that crate. Keep GUI timings separate from those tests, which have no renderer.

## Performance overlay

Press `Ctrl-Shift-F12` to toggle the overlay. It reports:

- GPUI frames drawn during the latest one-second sample
- average and 95th-percentile CPU draw time
- average invalidation-to-draw latency
- frame counts exceeding the 120 Hz and 60 Hz budgets

GPUI renders on demand, so an idle terminal can report zero or very low draw
FPS. This is not the monitor refresh rate or GPU presentation latency.

## Terminal-rendering workload

Launch the built-in workload:

```sh
zetta benchmark
```

From the repository, use an optimized build:

```sh
cargo run --release -- benchmark
```

The mode starts a deterministic 240 Hz full-grid producer and enables the
performance overlay. It is implemented by Zetta rather than a shell script, so
the same option works on Linux, macOS, and Windows. The workload intentionally
runs faster than common displays so frame coalescing and presentation overhead
remain visible.

The overlay provides application-level timings. For native stack samples,
attach the platform profiler while the workload runs: `perf` on Linux,
Instruments or `sample` on macOS, and Windows Performance Recorder/Analyzer on
Windows.

## Comparing other terminal emulators

Add `--profile-external-terminal` or `-x` to run only the deterministic
producer in the terminal that invoked Zetta, without opening a Zetta window.
Build once, then run the same optimized binary inside every terminal emulator:

```sh
cargo build --release
target/release/zetta benchmark -x -d 10
target/release/zetta benchmark -x -b -d 10
target/release/zetta benchmark -x -u -d 10
target/release/zetta benchmark -x -a -d 10
```

The commands run the standard 240 Hz grid, changing checkerboard, 40 Hz
sparse-update, and alternate-screen scroll workloads respectively. The workload
options select one pattern between them and cannot be combined. External mode
requires an explicit duration and restores terminal colors, cursor visibility,
and the normal screen when it exits.

Measure the hosting terminal emulator with the platform profiler or process
monitor during each run. Zetta cannot collect another application's frame
callbacks, so external mode cannot be combined with `--profile-report`.
Likewise, `--profile-pane-stress` remains Zetta-specific because an application
cannot create native panes in an unrelated terminal emulator.

## Automated reports

Run for ten seconds, write a portable JSON report, and exit:

```sh
zetta benchmark \
  --profile-report artifacts/zetta-performance.json
```

Set another duration, including fractional seconds, with
`--profile-duration`:

```sh
cargo run --release -- \
  benchmark \
  --profile-report artifacts/zetta-performance.json \
  --profile-duration 30
```

Providing a report path defaults to ten seconds. Outside external-terminal
mode, `--profile-duration` requires a report path. Zetta creates missing parent
directories, writes the report, and exits. Closing the window early or failing
to write the report returns a non-zero status.

The arguments are the same in PowerShell, Command Prompt, and Unix shells;
adjust line-continuation syntax when splitting the command.

Reports use a versioned JSON schema and include:

- Zetta version, build profile, operating system, and architecture
- logical CPU count and process CPU time for the hosting Zetta process
- average CPU utilization as a percentage of one logical core and of total
  machine capacity, both for each sample and for the complete run
- workload settings and requested and actual elapsed time
- per-second samples and total frame count
- draw FPS and average/p50/p95/p99 draw time
- average invalidation-to-draw latency
- counts exceeding the 120 Hz and 60 Hz frame budgets

`average_core_utilization_percent` uses 100% to mean one logical core. It is
the preferred value for comparing systems because it does not inherit the
different normalization used by Linux and Windows process monitors.
`average_machine_utilization_percent` divides that value by the reported
logical CPU count and is comparable to whole-machine-normalized tools such as
Windows Task Manager. CPU measurements cover only the hosting Zetta process;
the separate deterministic workload producer is excluded.

Preserve reports as CI artifacts or feed them into a separate comparison step.
Keep native stack traces as separate platform-profiler artifacts. Compare only
like-for-like optimized builds, workload settings, platforms, and GPU backends;
do not compare headless or software-rendered results with an interactive
hardware-rendered baseline.

## Multiplexer footprint

A flash-constrained host that only needs its sessions to stay alive is an
explicit use case for `zmux`, so the size of its minimal build is a property
worth measuring rather than assuming. Both optional paths — the retained
scrollback grid and the encrypted-on-disk store — are Cargo features, and the
`age` dependency tree is absent from a build without the second.

Measure from the standalone workspace, which is how a stripped-down daemon is
produced:

```sh
cd crates/zmux
cargo build --release --no-default-features --bin zmux
cargo build --release --no-default-features -F scrollback-buffer --bin zmux
cargo build --release --bin zmux
```

Record the stripped size of each — `strip` a copy, then `ls -l`, and
`size -A` for the `.text` figure. `cargo bloat` gives the per-crate breakdown
where it is installed.

Linux x86-64, Rust 1.95.0, recorded 2026-08-24:

| build | stripped | `.text` |
| --- | --- | --- |
| `--no-default-features` | 1.79 MiB | 1.28 MiB |
| `-F scrollback-buffer` | 2.13 MiB | 1.53 MiB |
| default (`scrollback-buffer`, `session-persistence`) | 6.43 MiB | 4.37 MiB |

The step from the second row to the third is the encryption stack — `age`,
`hpke` and the HTTP client that resolves `github:` recipients — which is the
cost persistence is opt-in to avoid. A regression in the first row is a
regression in the feature's purpose, so compare like-for-like release builds on
one platform, as with the frame-timing reports above.

The retained grid's own budget is separate from the binary: `sessions.retention`
of `memory` keeps at most `sessions.ring_bytes` per pane, 256 KiB by default, so
a daemon holding ten detached panes accounts for about 2.5 MiB of retained
screen plus each pane's PTY and metadata record. `none` allocates no grid at
all.

## WSL2 startup diagnostics

On Windows, WSL-backed terminals emit debug-level startup timing records. The
records use this format:

```text
WSL terminal startup phase=<phase> spawn_to_pty_ready_ms=<ms> pty_ready_to_marker_ms=<ms> total_ms=<ms>
```

The useful phases are:

- `pty_ready`: the WSL/ConPTY process was created and the PTY became ready.
- `first_shell_marker`: the first shell-integration marker arrived after the
  PTY was ready.
- `exit_before_first_shell_marker`: the terminal exited before shell
  integration produced its first marker.
- `subprocess_ready`: a headless/no-PTY WSL launch reached its subprocess-ready
  point.

`spawn_to_pty_ready_ms` includes WSL boot and process/PTY creation. The
`pty_ready_to_marker_ms` value measures shell startup and shell-integration
initialization after the PTY exists. Comparing those two values separates WSL
boot latency from slow shell startup. An exit-before-marker record identifies a
startup failure or an incomplete shell initialization rather than a normal
interactive shell exit.

These diagnostics do not change the WSL startup wrapper; they only measure the
existing launch and shell-integration phases. Collect the debug records with
the rest of the application's platform diagnostics when investigating slow or
unexpected WSL terminal startup.

## Windows shell-startup benchmark

Build an optimized binary, then run the developer benchmark from PowerShell 7:

```powershell
cargo build --release
./scripts/benchmark-shell-startup.ps1
```

The JSON report at `artifacts/shell-startup-performance.json` includes raw and
summarized cold-order/warm measurements for direct `zetta init`, Windows
PowerShell, PowerShell 7, Command Prompt, MSYS2 Bash and Zsh when installed,
and WSL as an unchanged control. It records median and p95 wall and CPU time,
process-tree I/O, child-process count, and the first `zetta-cwd` marker. Pass
`-Msys2Bash` or `-Msys2Zsh` when MSYS2 is outside the usual locations, and
`-SkipWsl` when no WSL control is needed.

The script deliberately does not purge Windows' file cache. Its cold-order
samples run before each case's explicit warmup and are useful for like-for-like
comparisons on the same machine; they are not a hardware cold-cache claim.

## Pane stress workload

Add `--profile-pane-stress` or `-s` to exercise multi-pane terminal rendering
while retaining the same producer, window, and capture settings. This creates
four visible panes, each running the deterministic producer:

```sh
cargo run --release -- \
  benchmark \
  -s \
  --profile-report artifacts/zetta-pane-stress.json \
  --profile-duration 10
```

Report metadata records `pane_count` and `minimized_pane_count`, distinguishing
ordinary and pane-stress runs without relying on file names. Because every pane
owns a PTY, parser, terminal grid, and rendered view, this mode measures actual
multi-terminal scaling rather than only pane-layout metadata.

## Background stress workload

Add `--profile-background-stress` or `-b` to replace the text workload with a
synthetic red-and-blue checkerboard made from alternating cell backgrounds.
Every cell switches between the two colors on each producer frame:

```sh
cargo run --release -- \
  benchmark \
  -b \
  --profile-report artifacts/zetta-background-stress.json \
  --profile-duration 10
```

This isolates terminal background-region collection, merging, and quad
painting while retaining the same 240 Hz producer and report format. Reports
record `workload.pattern` as either `standard` or `checkerboard_background`, so
the two workloads cannot be compared accidentally. The checkerboard is an
intentionally adverse case: no adjacent cells share a color, so every visible
colored cell requires its own paint quad.

Because it emits the most primitives per frame of any workload, it is the one
that exposes per-primitive costs in the scene. It is worth re-running after any
change to how the terminal paints: it was the workload that surfaced both the
bounds-tree insertion behind every unlayered quad and the wasted text run behind
every background-only cell.

## Sparse-update workload

Add `--profile-sparse-updates` or `-u` to populate a dense terminal once and
then change only a short status line at 40 Hz:

```sh
cargo run --release -- \
  benchmark \
  -u \
  --profile-report artifacts/zetta-sparse-updates.json \
  --profile-duration 10
```

This models full-screen TUIs with an animated spinner or streaming status line.
It exposes the cost of rebuilding and painting mostly unchanged terminal
content without conflating that cost with high PTY throughput. Reports record
`workload.pattern` as `sparse_updates` and `producer_hz` as `40`.

## Alternate-screen scroll workload

Add `--profile-alt-screen-scroll` or `-a` to scroll a colourised diff through
the alternate screen, a line at a time, repainting every visible row on every
producer frame:

```sh
cargo run --release -- \
  benchmark \
  -a \
  --profile-report artifacts/zetta-alt-screen-scroll.json \
  --profile-duration 10
```

This models `git diff` under a pager, which is the everyday case the plain-text
workloads do not represent: the alternate screen is active, every row's content
differs from the previous frame, and the rows carry the foreground colours,
bold spans, and occasional background highlight that a real diff produces. That
combination exercises the per-cell styling and text-run batching paths that the
uncoloured standard workload barely touches, without the adverse
every-cell-is-its-own-quad shape of the checkerboard. Reports record
`workload.pattern` as `alt_screen_scroll`.

## Linux and Wayland diagnostics

### WSLg graphics backend

Check which adapter Zetta actually selects before investigating WSLg throughput:

```sh
ZETTA_LOG=info zetta benchmark \
  --profile-report artifacts/wslg-default.json \
  --profile-duration 10
```

The `Selected GPU adapter` log line identifies the device and backend.
`llvmpipe` means rendering runs on the CPU. This can consume several cores
alongside PTY reading and parsing, even when Windows has a working GPU.
`glxinfo -B` and `vulkaninfo --summary`, when installed, also show which
devices Mesa exposes; check both because their device lists can differ.

If the default adapter is `llvmpipe`, test Mesa's WSL D3D12 driver with an
override scoped to one process:

```sh
GALLIUM_DRIVER=d3d12 ZETTA_LOG=info zetta benchmark \
  --profile-report artifacts/wslg-d3d12.json \
  --profile-duration 10
```

Confirm the log names a hardware device such as `D3D12 (AMD Radeon PRO
Graphics)` with the `Gl` backend. The override needs a working Mesa D3D12
driver and WSL GPU access; a successful launch alone does not establish that
hardware rendering was selected. Compare process CPU usage as well as frame
timings. Keep software and hardware results labelled by their selected
adapter, and use the same window size, workload, and visibility for both.

When comparing output throughput, apply the override to the terminal process
being launched (or its comparison script). Setting it in a shell inside an
already running terminal does not change that terminal's renderer.

### Stall diagnostics

Linux/Wayland release builds emit a `Zetta diagnostic:` line when a UI task,
terminal grid lock, or terminal snapshot construction stalls abnormally. The
watchdog is silent during normal operation and writes to standard error. After
a freeze, collect desktop-launch diagnostics with:

```sh
journalctl --user _COMM=zetta --since "15 minutes ago" --no-pager
```

The Wayland event-loop termination diagnostic includes the display and debug
forms of the underlying error. Preserve these lines alongside the performance
report when investigating a rendering stall.
