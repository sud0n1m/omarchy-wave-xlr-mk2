# Hardware EQ efficiency — v1.3.0

v1.3.0 adds the Wave XLR MK.2's onboard four-band EQ. Its state has 21 more
fields than v1.2.3. The goal was no runtime cost relative to v1.2.3 while
the device is connected, which had not been measured before.

## Method

The same Wave XLR MK.2 (firmware 1.0.15), connected, with no setting changes
during sampling. The bar's worker was stopped and each binary ran standalone
with `--watch`, after a `poll` command at 500 ms (panel closed) or 50 ms
(panel open). Each sample is 30 s after a 1 s settle. Binaries alternated,
three rounds per interval. CPU is on-CPU time from `/proc/<pid>/schedstat`
(nanoseconds, not scheduler ticks). Context switches are voluntary switches of
the worker. Output is bytes written to the shell on stdout. Release builds,
Rust 1.98, x86_64.

## Result

| Metric (mean of 3 × 30 s) | v1.2.3 | v1.3.0 |
|---|---:|---:|
| CPU, panel closed (500 ms) | 230 µs/s | 227 µs/s |
| CPU, panel open (50 ms) | 1670 µs/s | 1472 µs/s |
| Voluntary context switches/s, closed / open | 8.96 / 80.57 | 8.95 / 80.54 |
| Bytes to the shell, closed / open | 295 / 298 B/s | 45 / 46 B/s |
| Resident memory | 3380 KiB | 3451 KiB |
| Threads | 2 | 2 |

Raw samples: [final](hardware-eq/final.jsonl). Per-mapping memory is identical
except the executable's own pages (+20 KiB; the binary is 606928 bytes vs
596400). USB transfers per sample are unchanged (three blocks); the firmware
block is read once per connection.

## What made the first version slower, and the fixes

The first EQ build measured +60% CPU closed, +17% open, twice the output bytes
and +620 KiB resident memory ([samples](hardware-eq/first-cut.jsonl)):

- **Every sample decoded and compared ~40 JSON fields.** The worker now keeps
  the raw blocks behind the current state; an identical read skips decoding,
  comparison and cloning. Any other published state (failure, absence, ALSA,
  a write) clears the cache.
- **Every 1-second heartbeat re-sent the full state**, which the shell parsed
  and compared. Unchanged heartbeats are now liveness-only frames
  (`{"type":"state","seq":…,"sampledAtMs":…}`); full state is sent when it
  changes or a command (`poll`, `refresh`, a write) asks for it. The panel
  treats a frame without `state` as liveness.
- **Q conversion linked libm** (`pow`, `log10`), mapping 624 KiB of shared
  library pages. The 129 device Q values are now generated tables, and the
  nearest byte is found with geometric midpoints (`q² < Q(b)·Q(b+1)`), which
  equals `round(64·log10 q)`. A test sweeps Q values against the logarithm.

In the panel, the EQ page is a `Loader` that exists only while the page is
shown, so its 12 sliders add no bindings on the controls page or while the
panel is closed. Leaving the page mid-drag clears drag state and flushes the
final value, as closing the panel does.

Not measured: the shell process's own CPU (the reduction in parsed bytes and
bindings is structural) and wakeups while disconnected, which v1.3.0 does not
change (absence still sleeps on udev and stdin with no heartbeat).
