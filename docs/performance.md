# Performance profiling

Zetta provides an on-screen performance overlay, reproducible terminal-rendering
workloads, machine-readable reports, and platform diagnostics.

Always use an optimized build when recording or comparing measurements.

## Output throughput benchmark

Use the output benchmark to compare how quickly terminal emulators consume the
same text stream:

```sh
cargo build --release
target/release/zetta benchmark-output
```

The command writes exactly 10 MiB of deterministic, printable ASCII text to
standard output in 128 KiB blocks, flushes it, and then prints the elapsed time
and throughput to standard error. Payload construction is excluded from the
measurement, and the measured standard-output stream contains no timing
metadata.

Run the same optimized binary and command inside each terminal emulator. The
result measures the time for the process to write and flush the payload,
including terminal or PTY backpressure, like timing `cat` on a 10 MiB text
file. It does not measure when the terminal finishes presenting the last frame
on the GPU. Avoid redirecting standard output when comparing terminal
emulators, because that measures the redirected destination instead.

For a scrollback-scaling check, run the benchmark repeatedly in the same pane:

```sh
for run in 1 2 3 4 5 6 7 8 9 10; do
  target/release/zetta benchmark-output
done
```

Compare both the individual results and their trend. A fresh pane provides the
cold-history baseline; repeated runs reveal ingestion or rendering work that
grows with retained scrollback. Use the equivalent loop syntax in PowerShell
or Command Prompt on Windows.

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
zetta --profile-terminal-rendering
```

From the repository, use an optimized build:

```sh
cargo run --release -- --profile-terminal-rendering
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
target/release/zetta -P -x -d 10
target/release/zetta -P -x -b -d 10
target/release/zetta -P -x -u -d 10
```

The commands run the standard 240 Hz grid, changing checkerboard, and 40 Hz
sparse-update workloads respectively. External mode requires an explicit
duration and restores terminal colors and cursor visibility when it exits.

Measure the hosting terminal emulator with the platform profiler or process
monitor during each run. Zetta cannot collect another application's frame
callbacks, so external mode cannot be combined with `--profile-report`.
Likewise, `--profile-pane-stress` remains Zetta-specific because an application
cannot create native panes in an unrelated terminal emulator.

## Automated reports

Run for ten seconds, write a portable JSON report, and exit:

```sh
zetta --profile-terminal-rendering \
  --profile-report artifacts/zetta-performance.json
```

Set another duration, including fractional seconds, with
`--profile-duration`:

```sh
cargo run --release -- \
  --profile-terminal-rendering \
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

## Pane stress workload

Add `--profile-pane-stress` or `-s` to exercise multi-pane terminal rendering
while retaining the same producer, window, and capture settings. This creates
four visible panes, each running the deterministic producer:

```sh
cargo run --release -- \
  --profile-terminal-rendering \
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
  --profile-terminal-rendering \
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

## Sparse-update workload

Add `--profile-sparse-updates` or `-u` to populate a dense terminal once and
then change only a short status line at 40 Hz:

```sh
cargo run --release -- \
  --profile-terminal-rendering \
  -u \
  --profile-report artifacts/zetta-sparse-updates.json \
  --profile-duration 10
```

This models full-screen TUIs with an animated spinner or streaming status line.
It exposes the cost of rebuilding and painting mostly unchanged terminal
content without conflating that cost with high PTY throughput. Reports record
`workload.pattern` as `sparse_updates` and `producer_hz` as `40`.

## Linux and Wayland diagnostics

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
