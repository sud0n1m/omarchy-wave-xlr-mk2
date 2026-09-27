# Background efficiency review — 2026-09-27

Deployed plugin v1.1.1. The healthy USB path keeps one Python worker, sampling
all three small device blocks every 500 ms closed / 50 ms open. Polling input
interrupts its selector immediately. No change to audio streaming or DSP settings.

Changes:

- Removed the worker's unconditional 100 ms selector timeout. Sleep ends at the
  next sample or heartbeat deadline, or immediately on a command.
- Replaced the UI's repeating 250 ms housekeeping timer with one-shot watchdog
  and highlight-expiry timers. No highlight work while closed.
- Identical heartbeats refresh liveness without replacing control state objects.
  Repeated snapped slider values do not issue writes; release still flushes a
  queued final value. Removed redundant refresh commands after poll commands.
- Cache libusb library discovery to avoid repeated ldconfig processes on retries.
- Permission-only ALSA fallback reads all three controls with one amixer process
  instead of three. Closed polling is every 2 seconds (previously 500 ms), reducing
  read subprocesses from 6/second to 0.5/second. Open fallback remains 500 ms.
- Missing hardware waits for reconnect backoff while retaining 1-second heartbeats.

## Measurement

Same workstation, physical device connected, panel closed, 20 seconds each:

| Worker metric | Before | After |
|---|---:|---:|
| CPU, percent of one core | 0.10% | 0.05% |
| Resident memory | 16.84 MiB | 16.96 MiB |
| Main-thread voluntary context switches/second | 15.95 | 8.80 |
| Threads | 2 | 2 |

[Before](before.json), [after](after.json).
Context switches decreased about 45%; they include USB waits and are **not**
machine-wide wakeup counts. CPU uses coarse scheduler ticks over a short sample;
these results do not establish a precise CPU reduction or battery-life estimate.
RSS is for the worker, not the shared Omarchy shell. UI savings were verified by
code/tests rather than isolated shell CPU attribution. An initial after-measurement
was discarded because editing plugin files triggered a reload during sampling.

## Verification

- 20 backend tests passed, including idle deadlines, heartbeat during backoff,
  fallback rate, single-process ALSA reads, and cached library discovery.
- Seven actual-QML slider gesture cases passed (8 Qt tests including initialization).
- Actual panel functions tested in a JS VM: no-op edits, final flush, unchanged
  heartbeat object identity, and propagation of changed device values.
- Live `amixer contents` parsing returned the correct gain, mute and headphone values.
- Froze only the worker to check the new watchdog: recovered in 4.61 seconds,
  one replacement worker, all settings preserved, no error. [Result](recovery.json).
- Opened the deployed panel and verified fresh USB state via its native IPC;
  then closed it. [Status](final-status.json). No recent matching QML errors.
- Live files copied to the repository; restore instructions updated.

The 50 ms open polling setting is unchanged. This review did not remeasure physical
knob-to-screen latency or alter gain, phantom power, or other hardware settings.
