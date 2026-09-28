# Event-only unplugged idle — v1.2.1

When the Wave XLR is physically absent, the worker now calls `poll()` with an
infinite timeout. It sends its initial absent state, then emits no periodic
heartbeats and performs no periodic device checks. It wakes for a filtered udev
USB-device notification, an explicit UI command, stdin closure, or a signal.

The QML panel stops its watchdog only after absence is confirmed and all pending
commands have replies. Sending a command arms a timeout, even when unplugged.
When the device appears, the worker emits a present/connecting state before any
potentially blocking USB operation so the panel can restart its watchdog.
Connected polling, heartbeats, readback checks and reconnect backoff remain intact.
Device-present failures are marked present/stale instead of inheriting an old
absence flag.

Process exits remain observable by Quickshell without a timer. A living worker
that becomes unresponsive while parked is checked on the next UI command; there
is deliberately no periodic check for that condition. Opening the panel is enough
to initiate recovery. The device-arrival wake notice protects subsequent USB work.

## Live verification

Device unplugged, panel closed, 20 seconds on the workstation:

- Zero scheduler CPU ticks.
- Zero voluntary context switches.
- Zero read and write syscalls.
- No new state messages; no active UI watchdog or outstanding commands.
- Worker sleeping in `poll()`, RSS 2.86 MiB.

[Recorded idle sample](event-only/idle.json). This is a bounded observation, not a
claim that the process can never be scheduled by the OS or external events.

The test deliberately stopped the worker. It remained parked with no timer;
opening the panel then recovered it in 5.58 seconds with one replacement worker
and no error. [Command recovery](event-only/command-recovery.json).
Killing the idle worker triggered process-exit recovery in 2.43 seconds.
[Exit recovery](event-only/exit-recovery.json).

37 Rust tests pass, including no heartbeat/deadline while absent, waking the UI
before opening USB, command replies during absence, and timed recovery for a busy
attached device. Actual panel JS tests cover command IDs, unknown acknowledgments,
watchdog pause/resume, defaults deadlines, and control-state identity. Seven
actual-QML slider gesture tests still pass; Clippy passes with warnings denied.
Physical redocking and connected Rust hardware writes remain unverified while the
laptop is intentionally undocked. Marketplace submission remains pending that check.
