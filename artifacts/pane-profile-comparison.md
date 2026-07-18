# Pane-management graphical profile comparison

Recorded 2026-07-18 using Zetta's automated 240 Hz, 34-row terminal workload.
The pre-optimization scenarios were captured three times for ten seconds. The
post-optimization scenarios were captured three times for two seconds,
alternating baseline and stress runs. The shorter post-change captures avoid a
GNOME Shell automation behavior that occluded repeated profiling windows after
several seconds; every retained per-second sample contains 59 to 62 frames.

Environment:

- Linux 6.17.0-40-generic, GNOME Shell 46.0, Wayland
- Intel Core Ultra 7 265H, 16 logical CPUs
- Intel Arrow Lake-P integrated graphics
- 1920x1200 display at 59.88 Hz
- Identical 1100x720 Zetta windows and hardware-rendered Wayland backend

Scenarios:

- Baseline: one visible pane, no minimized panes
- Stress: 64-pane layout, one visible pane, 63 minimized panes

## Before optimization

| Aggregate across three 10-second runs | Baseline | Stress | Change |
| --- | ---: | ---: | ---: |
| Frames | 1,790 | 1,790 | 0 |
| Draw FPS | 59.573 | 59.577 | +0.006% |
| Average draw | 1.503 ms | 2.443 ms | +62.6% |
| p50 draw | 1.283 ms | 2.047 ms | +59.5% |
| p95 draw | 3.145 ms | 5.454 ms | +73.4% |
| p99 draw | 4.354 ms | 6.278 ms | +44.2% |
| Average invalidation-to-draw latency | 15.480 ms | 15.752 ms | +1.8% |
| Frames over 8.33 ms | 5 | 7 | +2 |
| Frames over 16.67 ms | 0 | 1 | +1 |

Raw reports:

- `pane-profile-baseline-1.json`
- `pane-profile-baseline-2.json`
- `pane-profile-baseline-3.json`
- `pane-profile-stress-1.json`
- `pane-profile-stress-2.json`
- `pane-profile-stress-3.json`

Reproduction commands:

```sh
target/release/zetta --profile-terminal-rendering \
  --profile-report artifacts/pane-profile-baseline-N.json \
  --profile-duration 10

target/release/zetta --profile-terminal-rendering \
  --profile-pane-stress \
  --profile-report artifacts/pane-profile-stress-N.json \
  --profile-duration 10
```

Assessment: the stress state preserves display-rate throughput, but its
material increase in draw CPU time and tail latency confirms that the current
pane-management render path scales poorly near the 64-pane limit. It is not a
freeze at this refresh rate, but the layout derivation and shelf construction
should be optimized before treating the upper limit as routine use.

## After optimization

| Mean across three 2-second runs | Baseline | Stress | Change |
| --- | ---: | ---: | ---: |
| Frames | 362 | 361 | -1 |
| Draw FPS | 60.117 | 59.694 | -0.7% |
| Average draw | 1.413 ms | 1.563 ms | +10.6% |
| p50 draw | 1.252 ms | 1.360 ms | +8.6% |
| p95 draw | 2.424 ms | 1.961 ms | -19.1% |
| p99 draw | 3.886 ms | 9.269 ms | +138.5% |
| Average invalidation-to-draw latency | 15.324 ms | 15.505 ms | +1.2% |
| Frames over 8.33 ms | 3 | 6 | +3 |
| Frames over 16.67 ms | 0 | 0 | 0 |

Raw post-optimization reports:

- `pane-profile-final-2s-baseline-1.json`
- `pane-profile-final-2s-baseline-2.json`
- `pane-profile-final-2s-baseline-3.json`
- `pane-profile-final-2s-stress-1.json`
- `pane-profile-final-2s-stress-2.json`
- `pane-profile-final-2s-stress-3.json`

The optimized path removes all minimized panes from the layout in one tree
traversal and renders a constant-size shelf containing only the selected pane,
with previous/next controls and a position count. The average stress overhead
fell from 62.6% to 10.6%, and median overhead fell from 59.5% to 8.6%, while
maintaining display-rate throughput. The p99 difference represents a few
isolated frames in short runs rather than sustained slow rendering: neither
scenario produced a frame over 16.67 ms.
