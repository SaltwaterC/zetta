# Performance profiling

Zetta provides an on-screen performance overlay, reproducible terminal-rendering
workloads, machine-readable reports, and platform diagnostics.

Always use an optimized build when recording or comparing measurements.

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

Providing a report path defaults to ten seconds. `--profile-duration` requires
a report path. Zetta creates missing parent directories, writes the report, and
exits. Closing the window early or failing to write the report returns a
non-zero status.

The arguments are the same in PowerShell, Command Prompt, and Unix shells;
adjust line-continuation syntax when splitting the command.

Reports use a versioned JSON schema and include:

- Zetta version, build profile, operating system, and architecture
- workload settings and requested and actual elapsed time
- per-second samples and total frame count
- draw FPS and average/p50/p95/p99 draw time
- average invalidation-to-draw latency
- counts exceeding the 120 Hz and 60 Hz frame budgets

Preserve reports as CI artifacts or feed them into a separate comparison step.
Keep native stack traces as separate platform-profiler artifacts. Compare only
like-for-like optimized builds, workload settings, platforms, and GPU backends;
do not compare headless or software-rendered results with an interactive
hardware-rendered baseline.

## Pane stress workload

Add `--profile-pane-stress` or `-s` to exercise pane-management rendering while
retaining the same producer, window, and capture settings. This creates 64
panes, minimizes 63, and leaves the profiler terminal visible:

```sh
cargo run --release -- \
  --profile-terminal-rendering \
  -s \
  --profile-report artifacts/zetta-pane-stress.json \
  --profile-duration 10
```

Report metadata records `pane_count` and `minimized_pane_count`, distinguishing
ordinary and pane-stress runs without relying on file names.

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
