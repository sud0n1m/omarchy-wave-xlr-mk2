# Elgato Wave XLR MK.2 for Omarchy

<img src="icon.svg" width="64" height="64" alt="Elgato Wave XLR MK.2 device icon">

Linux USB audio interface controls for **Elgato Wave XLR MK.2** (also written
Wave XLR MK2): microphone gain, mute, headphones, monitor mix and onboard DSP,
in a native Omarchy shell plugin. This supports the MK.2, not the original Wave
XLR, Wave XLR Pro or Stream Deck XLR Dock. Click the outlined
Wave XLR device icon beside the stock audio control. The icon follows the bar
color, dims on loss of connection, and uses the theme's urgent color plus a slash
when muted. The panel inherits the live Omarchy theme, font, spacing and scale.

Built with native Omarchy controls: a header that never scrolls away, prominent gain,
monitoring, a grid of onboard-effect switches, and Equalizer and Hardware pages
reached from matching rows (the Equalizer row previews the current curve). Some native details (square
switches, borders, sizing) intentionally follow the shell rather than the mockup.

![Elgato Wave XLR MK.2 controls and device icon inheriting the Omarchy theme](preview.png)

## Controls

- Actual preamp gain 0–80 dB, hardware capture mute, headphone attenuation
  −60 to 0 dB. Gain keys step 1 dB; headphone keys step 0.25 dB.
- Vendor USB controls: Clipguard, low cut, expander, compressor, Voice Tune and
  strength, direct monitor blend (0 mic, 100 center, 200 computer), low impedance,
  and 48V phantom power with explicit confirmation. Cancel receives focus first.
- Wheel steps headphones by 1 dB; Shift-wheel by 0.25 dB. Tab/Shift-Tab navigate,
  arrows adjust sliders, M toggles mute, Escape cancels/backs out/closes.
- Hardware EQ (firmware 1.0.15 or later): the four-band equalizer Wave Link
  runs on the unit's own processor. Master switch, and per band: on/off, filter
  type (high-pass, low shelf, bell, notch, high shelf, low-pass), frequency
  20 Hz–20 kHz, gain ±12 dB in 0.25 dB steps, and Q 0.1–10. The page draws the
  summed response of the enabled bands (RBJ biquads at 48 kHz) with a numbered
  point per band: drag a point to set frequency and gain, scroll to change Q.
  Band tabs pick which band the editor below shows. Its frequency and Q
  sliders are logarithmic; arrows and the wheel step 1/12 octave and one
  device Q step. Types that ignore gain show no gain control. Bands set in
  Wave Link are shown and kept. Wave Link leaves the EQ running when it quits,
  so a unit last used on Windows or macOS may arrive with it switched on.
- The Hardware page shows the firmware version and contains the explicit “Set as default”
  action for system audio. Loading the plugin changes neither defaults nor hardware settings.
- Help explains controls and keyboard behavior. Values are settings, not meters.
- Compressor/expander parameter tuning (threshold, ratio, attack, release),
  LED customization, Auto Gain, gain-lock policy, firmware upgrades, Wave Link
  application mixing and VST hosting are not implemented. The compressor and
  expander byte layout is only inferred from the Wave:3 MK.2 for this model.

## Responsive device state

A single persistent Rust worker owns the vendor USB interface. The panel uses
newline-delimited JSON on stdin/stdout, rather than spawning a process per poll.
Device-side changes are event-driven. The MK.2 reports gain, mute and
headphone changes (knob, mute button or any writer) on its audio-control
interrupt endpoint; the worker subscribes to the card's ALSA control events
through the kernel interface (no libasound) and reads only the reported block
within a few milliseconds. With events active and the panel closed, the
worker does nothing until the device, a command or a hotplug wakes it: no
polling and no heartbeat (0 wakeups/s idle, against about 9 in v1.2.3). The
open panel also reads the two blocks the dial can change without an event
(monitor blend, Voice Tune strength) every 50 ms, and every block every 5 s.
Without events (no card access), it falls back to reading every block every
50 ms open and 500 ms closed.

The worker sends a liveness frame at least once per second while hardware is
present, except while quiet: with events active and the panel closed, its
frames carry `"quiet": true` and the shell stops its watchdog until a command
is outstanding or a frame without the flag arrives. A full `state` frame is sent on `poll`, `refresh`, reconnection and
errors; between steady connected states only the changed fields are sent as
`patch`, and an unchanged heartbeat carries neither. Command results carry no
state, because every change was published as a frame before them. An
unchanged USB read is not decoded again. Writes read, modify and read back
only the target block ([v1.3.0 measurements](docs/hardware-eq-efficiency.md)). Incoming
reads never disable sliders. Local drags display immediately, coalesce writes
at 50 ms, and flush the final release. Pending IDs protect newer edits from older
acknowledgments. Device-side changes briefly highlight the affected number.
Native Omarchy slider thumbs retain 140 ms easing; numeric values update directly.

Stale values are explicitly marked and mutations disabled. A stalled worker is
restarted after a 3-second watchdog (12 seconds during the bounded defaults
operation); reconnect uses backoff, without blindly replaying writes. The ALSA
fallback is available only when the device is present but USB permission is absent.
Only one worker can own the lock; the standalone CLI fails fast while it runs.
The current deployment is a single-monitor bar; multi-instance ownership is not
implemented and must be addressed before enabling duplicate bar instances.

Background efficiency (v1.2.1): while connected, the worker blocks until the next
500 ms device check or 1-second heartbeat; there is no extra 100 ms wake loop.
The UI uses one-shot watchdog/highlight timers, ignores identical state for
control bindings, and suppresses writes when the snapped value has not changed.
When physically disconnected, a compact panel replaces the controls. One initial
sysfs inventory and passive udev notifications track attachment; there are no USB
open attempts, control reads, ALSA subprocesses, or periodic inventory scans while
absent. After announcing absence, the worker blocks indefinitely on its device
notification socket and stdin: no heartbeat or timer wakeups. The panel stops its
watchdog while idle and absent, but still detects process exits and starts a
bounded timeout for each user command. A device-arrival notice restarts the UI
watchdog before the worker opens USB. Controls return after attachment; reconnect backoff applies only while hardware is present
but unavailable. libusb is linked directly. Permission-only
ALSA fallback reads all basic controls with one process, every 2 seconds while
closed or 500 ms while open. The normal USB path starts no polling subprocesses.

Read-only diagnostics while the bar is running:

```
omarchy-shell sudonim.wave-xlr-status status
```

The standalone `libexec/wave-xlr-control` still works with the plugin disabled and no
worker holding the lock. Do not run another vendor-control app concurrently.

## Protocol and safety

Mapping reference: [OpenXLR WaveXlrMk2Device.cs](https://github.com/emaspa/openxlr/blob/main/src/OpenXLR.Core/Devices/WaveXlrMk2Device.cs).
Vendor/product 0fd9:00b6, interface 3, bank 0x0203, blocks 1/4/5 (6/38/2 bytes).
Block 0 (117 bytes) is read once per connection; bytes 5–7 are the firmware
version. From firmware 1.0.15, block 4 also carries the EQ: byte 2 bit 0 is
the master switch, and four 5-byte bands start at byte 18 (flags with bit 0 on
and bits 1–3 the type, int8 gain in quarter dB, uint16 LE frequency, int8 Q as
round(64·log10 Q)). EQ controls appear only when the firmware is new enough
and every band decodes within Wave Link's ranges; otherwise the state reports
`eqUnavailable` (`firmwareOld`, `firmwareUnknown` or `unsupportedBlock`) and
EQ writes are refused before any transfer. Band edits keep the flag byte's
unknown bits 4–7. EQ mapping reference: [OpenXLR MK.2 EQ findings](https://github.com/emaspa/openxlr/blob/main/docs/mk2-eq-findings.md).
The Rust/libusb worker claims only the vendor interface and never detaches the
audio driver. Writes read the current block, modify only the chosen field,
preserve unknown bytes, and confirm the selected value by readback. Unexpected
lengths/ranges fail closed. These are community protocol mappings, not an Elgato
Linux API. Register readback confirms state, not acoustic processing quality.

## Installation

Runtime dependencies: libusb, systemd/libudev, ALSA tools, PipeWire/WirePlumber, and the Omarchy
Quickshell shell. Build dependencies: Rust/Cargo, a C compiler, and pkg-config.
Python is not required for installation, runtime, or tests.

Install source, compile the worker once, then enable the plugin:

```bash
omarchy pkg add rust base-devel pkgconf libusb alsa-utils
omarchy plugin add https://github.com/sud0n1m/omarchy-wave-xlr-mk2
~/.config/omarchy/plugins/sudonim.wave-xlr/build-worker
omarchy plugin enable sudonim.wave-xlr
```

`build-worker` runs `cargo build --release --locked` and installs a native worker
in the plugin's `libexec/` directory. Build output is cached outside the plugin
in `~/.cache/omarchy-wave-xlr-mk2/build` (respects `XDG_CACHE_HOME`). There is no
runtime compilation, Python fallback, downloaded executable, or background cargo
process. Cargo downloads the source dependencies recorded in `Cargo.lock` during
the explicit build. No root privileges are needed to compile.

After updating the source, rebuild and restart the shell:

```bash
omarchy plugin update sudonim.wave-xlr
~/.config/omarchy/plugins/sudonim.wave-xlr/build-worker
omarchy restart shell
```

### Manual USB setup

Basic gain, mute and headphone controls can use ALSA without vendor USB access.
For onboard DSP and the full control panel, use the ownership-checking setup
command after building the worker. Run these commands in a terminal; setup
requests administrator authorization explicitly, never from the background worker:

```bash
~/.config/omarchy/plugins/sudonim.wave-xlr/libexec/wave-xlr-control --install-udev-rule
sudo udevadm control --reload-rules
```

Reconnect the Mk.2. The rule grants the active local user access only to this
product. Do **not** prefix the worker command with `sudo`. The unprivileged helper
hashes its running executable through `/proc/<pid>/exe` before the password
prompt. It invokes only the system `sudo` and `bash` at the privilege boundary,
with a literal bootstrap supplied before authentication. System tools copy the
running executable to a private root-owned directory, verify its SHA-256 against
the pre-prompt digest, then execute that verified copy. A changed copy is rejected;
the user-writable checkout is never executed directly as root. The private copy
is removed on exit. This uses the system Bash, coreutils and sudo already present
on Omarchy.

The helper creates `70-sudonim-wave-xlr-mk2.rules` under
`/etc/udev/rules.d/` using an atomic operation that cannot overwrite an existing
path. A root-only `.sudonim-wave-xlr-mk2/` directory beside it records ownership
with a hard link to the installed inode. Repeated setup accepts only that exact
inode with unchanged contents, owner and permissions. Existing files (even with
identical contents), symlinks, modified rules and replacements are preserved and
reported as conflicts. The helper does not claim USB or contact the device.

**Upgrading from 1.2.1 or earlier:** the old `70-wave-xlr-mk2.rules` path is left
untouched, since ownership cannot be established. An existing rule can continue
providing access. Do not delete or overwrite it merely because of its filename;
review its provenance or use its owning package/tool to manage it.

Tested with Omarchy's Quickshell-based shell; this is not a Waybar
module. The plugin claims only the vendor control interface; the kernel keeps the audio streaming interface.

Optional placement and connection check:

```bash
omarchy bar put sudonim.wave-xlr --before omarchy.audio
omarchy restart shell
omarchy-shell sudonim.wave-xlr-status status
```

The explicit shell restart ensures no cached QML/IPC instance remains from an
older version. Status should report `usb: true`, `stale: false`, one running
worker, and no error. The worker is owned by the shell; no systemd unit is needed.
Do not install the Mk.1 WirePlumber workaround for this Mk.2.

## Removal

If you installed this version's USB rule and no other Wave XLR MK.2 tool needs
it, remove it **before** removing the plugin (the native helper is needed):

```bash
~/.config/omarchy/plugins/sudonim.wave-xlr/libexec/wave-xlr-control --remove-udev-rule
sudo udevadm control --reload-rules
```

Removal requires the original inode plus matching receipt, contents, owner and
permissions. It refuses to delete an unowned, modified, replaced or symlinked
rule. Do not bypass a refusal with `rm`; leave the rule for its owner to review.
The empty private ownership directory and setup lock remain for safe concurrent
setup; neither grants device access. The old generic rule is never removed.

```bash
omarchy plugin remove sudonim.wave-xlr
```

Reconnect the device to apply a permission change. Removing the plugin leaves
hardware settings and any explicitly selected PipeWire defaults unchanged.

## Verification

```bash
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
# Optional UI integration tests: Node.js + Omarchy/Quickshell + QtTest required
cargo test --locked --test panel -- --ignored --nocapture
```

The Rust tests use fake devices and clocks for protocol preservation (every EQ
field against every other byte and bit, Q encoding for all 129 device values,
the firmware gate and corrupt bands), bounds,
readback, reconnects, ALSA fallback, stale-state handling, and deadlines. Framing
and subprocess tests cover oversized/invalid input, timeout cleanup, and output
larger than a pipe buffer. These tests never write to real hardware.
Nine setup tests cover ownership, collisions (including identical files),
modified/replaced rules, symlinks, permissions, concurrent setup and interrupted
installation, using isolated temporary directories without administrator access.
The staging test replaces an executable's checkout pathname after hashing and
verifies that the pinned running image is still used, rejects a wrong digest,
and checks private-copy cleanup. It runs a harmless fixture without sudo.

The optional UI tests are run by Rust and exercise JavaScript functions and slider
components extracted from the actual `Panel.qml`, including the logarithmic
frequency and Q sliders, dragging and scrolling EQ graph points, and the EQ
response math against known filter values (bell peak, −3 dB cutoffs, shelf
plateaus, notch depth). The QML tests use an isolated
offscreen shell and never start the device worker. These three tests are explicitly
ignored by default so the worker suite can run without desktop dependencies.

The original Python implementation and its tests are available in Git history.
The current source and test runners contain no Python. Historical Python
measurements are in [the efficiency review](docs/efficiency.md). See
[the Rust migration report](docs/rust-migration.md) for current verification and
measurement limits. [Event-only idle verification](docs/event-only-idle.md) covers
v1.2.1, which removes the unplugged heartbeat. Physical-knob-to-screen latency and acoustic DSP quality
have not been measured.

## License and acknowledgments

[MIT](LICENSE). Thanks to [OpenXLR](https://github.com/emaspa/openxlr) for documenting
the Wave XLR MK.2 USB protocol. This project implements those hardware mappings
in its own control worker and does not bundle or execute OpenXLR.
The device-outline icon is original artwork for this community plugin, shared
by the bar and panel heading. This project is not affiliated with Elgato.
