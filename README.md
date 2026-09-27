# Wave XLR MK.2 for Omarchy

An Omarchy shell plugin for Elgato Wave XLR MK.2. Click the outlined
Wave XLR device icon beside the stock audio control. The icon follows the bar
color, dims on loss of connection, and uses the theme's urgent color plus a slash
when muted. The panel inherits the live Omarchy theme, font, spacing and scale.

Built with native Omarchy controls: prominent gain, monitoring, aligned onboard-effect
switches, and a separate hardware-settings page. Some native details (square
switches, borders, sizing) intentionally follow the shell rather than the mockup.

## Controls

- Actual preamp gain 0–80 dB, hardware capture mute, headphone attenuation
  −60 to 0 dB. Gain keys step 1 dB; headphone keys step 0.25 dB.
- Vendor USB controls: Clipguard, low cut, expander, compressor, Voice Tune and
  strength, direct monitor blend (0 mic, 100 center, 200 computer), low impedance,
  and 48V phantom power with explicit confirmation. Cancel receives focus first.
- Wheel steps headphones by 1 dB; Shift-wheel by 0.25 dB. Tab/Shift-Tab navigate,
  arrows adjust sliders, M toggles mute, Escape cancels/backs out/closes.
- Hardware settings contains the explicit “Use mic + headphones as defaults”
  action. Loading the plugin changes neither defaults nor hardware settings.
- Help explains controls and keyboard behavior. Values are settings, not meters.
- EQ, LED customization, Auto Gain, gain-lock policy, firmware upgrades,
  Wave Link application mixing and VST hosting are not implemented.

## Responsive device state

A single persistent Python worker owns the vendor USB interface. The panel uses
newline-delimited JSON on stdin/stdout, rather than spawning a process per poll.
It reads every 50 ms while open and 500 ms while closed, sending changed state
plus a liveness frame at least once per second in normal operation. Incoming
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

Background efficiency (v1.1.1): the worker blocks on stdin until the next
500 ms device check or 1-second heartbeat; there is no extra 100 ms wake loop.
The UI uses one-shot watchdog/highlight timers, ignores identical state for
control bindings, and suppresses writes when the snapped value has not changed.
Missing devices use reconnect backoff; library discovery is cached. Permission-only
ALSA fallback reads all basic controls with one process, every 2 seconds while
closed or 500 ms while open. The normal USB path starts no polling subprocesses.

Read-only diagnostics while the bar is running:

```
omarchy-shell sudonim.wave-xlr-status status
```

The standalone `python3 control.py` still works with the plugin disabled and no
worker holding the lock. Do not run another vendor-control app concurrently.

## Protocol and safety

Mapping reference: [OpenXLR WaveXlrMk2Device.cs](https://github.com/emaspa/openxlr/blob/main/src/OpenXLR.Core/Devices/WaveXlrMk2Device.cs).
Vendor/product 0fd9:00b6, interface 3, bank 0x0203, blocks 1/4/5 (6/38/2 bytes).
The ctypes/libusb worker claims only the vendor interface and never detaches the
audio driver. Writes read the current block, modify only the chosen field,
preserve unknown bytes, and confirm the selected value by readback. Unexpected
lengths/ranges fail closed. These are community protocol mappings, not an Elgato
Linux API. Register readback confirms state, not acoustic processing quality.

## Restore

Dependencies: Python 3, libusb, ALSA tools, PipeWire and the Omarchy shell.
From the repository root:

```bash
mkdir -p ~/.config/omarchy/plugins/sudonim.wave-xlr
cp Panel.qml control.py manifest.json ~/.config/omarchy/plugins/sudonim.wave-xlr/
sudo install -m 0644 udev/70-wave-xlr-mk2.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
```

Reconnect the Mk.2, then enable the plugin below. The rule grants the active
local user access only to this product. Tested with Omarchy's Quickshell-based
shell; this is not a Waybar module.

```
omarchy plugin validate ~/.config/omarchy/plugins/sudonim.wave-xlr
omarchy-shell shell rescanPlugins
omarchy bar put sudonim.wave-xlr --before omarchy.audio
omarchy restart shell
omarchy-shell sudonim.wave-xlr-status status
```

The explicit shell restart ensures no cached QML/IPC instance remains from an
older version. Status should report `usb: true`, `stale: false`, one running
worker, and no error. The worker is owned by the shell; no systemd unit is needed.
Do not install the Mk.1 WirePlumber workaround for this Mk.2.

## Verification

Run `python3 -m unittest -v test_control test_watch` in the plugin directory.
Run `python3 tests/test_panel_efficiency.py` (Node.js required for this test only)
to check unchanged-state handling and final-edit flushing.
Also run `python3 tests/test_panel_slider.py`: seven gesture cases load the actual
SliderField in isolated offscreen Quickshell without touching USB. They verify
single wheel delivery, fine steps, drag snapping, external updates and cancellation.

20 backend tests cover protocol preservation/bounds, NDJSON parsing, locking, liveness,
fallback/reconnect, idle scheduling, cached library discovery and failed-write behavior. Live keyboard checks verified
43 → 42 → 43 dB gain and −20 → −20.25 → −20 dB headphones. Phantom Cancel and
Escape preserve 48V and restore focus. Watchdog recovery was verified by freezing
only the worker; it recovered in 4.9 seconds with one owner and unchanged settings.

External ALSA gain changes reached the worker in about 50.4 ms (8 samples).
This is **not** measured physical-knob-to-screen latency. The latter and acoustic
DSP quality remain unverified. See the [efficiency review](docs/efficiency.md) for recorded measurements.
