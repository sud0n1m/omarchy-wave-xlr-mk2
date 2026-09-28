"""No hardware required: exercise the serialized owner using a fake USB handle."""
import unittest
from unittest.mock import patch
import control
import fcntl
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from control import Watch, LENGTHS

class FakeDevice:
    def __init__(self):
        self.blocks = {k: bytearray(n) for k, n in LENGTHS.items()}
        self.blocks[4][0] = 43
        self.writes = []
        self.closed = False
        self.fail_write = False
        self.concurrent = False

    def read(self):
        return {k: bytearray(v) for k, v in self.blocks.items()}

    def transfer(self, block, data):
        if self.fail_write:
            raise RuntimeError('Device disconnected during write')
        self.writes.append((block, bytes(data)))
        self.blocks[block] = bytearray(data)
        if self.concurrent:
            self.blocks[4][10] = 12

    def close(self):
        self.closed = True

class WatchTests(unittest.TestCase):
    def setUp(self):
        self.now = 1.
        self.messages = []
        self.device = FakeDevice()
        self.owner = Watch(self.messages.append, device_factory=lambda: self.device,
                           clock=lambda: self.now, present=lambda: False)
        self.owner.sample(force=True)

    def command(self, **kwargs):
        self.owner.command(dict(id=42, **kwargs))
        return self.messages[-1]

    def test_initial_state_poll_rate_and_liveness(self):
        self.assertEqual(self.messages[-1]['state']['gain'], 43)
        self.assertEqual(self.messages[-1]['sampledAtMs'], 1000)
        self.assertEqual(self.owner.interval, .5)
        self.assertTrue(self.command(op='poll', intervalMs=50)['ok'])
        self.assertEqual(self.owner.next_poll, 1.05)
        count = len(self.messages)
        self.now = 1.05
        self.owner.tick()
        self.assertEqual(len(self.messages), count)  # unchanged samples suppressed
        self.now = 2.01
        self.owner.tick()
        self.assertEqual(self.messages[-1]['type'], 'state')
        self.assertEqual(self.messages[-1]['sampledAtMs'], int(self.now * 1000))
        self.assertEqual(self.messages[-1]['seq'], 3)

    def test_idle_sleep_and_disconnected_heartbeat_deadlines(self):
        self.assertAlmostEqual(self.owner.wait_timeout(), .5)
        self.owner.next_poll = 6
        self.now = 1.4
        self.assertAlmostEqual(self.owner.wait_timeout(), .6)
        self.now = 2
        self.owner.tick()
        self.assertEqual(self.messages[-1]['type'], 'state')
        self.assertAlmostEqual(self.owner.wait_timeout(), 1)

    def test_missing_device_waits_for_retry_without_losing_heartbeat(self):
        calls = []
        def missing():
            calls.append(self.now)
            raise RuntimeError('disconnected')
        self.owner.close()
        self.owner.device_factory = missing
        self.owner.backoff = 5
        self.owner.sample()
        self.assertEqual(self.owner.next_poll, 6)
        for instant in [2, 3, 4, 5]:
            self.now = instant
            self.owner.tick()
            self.assertAlmostEqual(self.owner.wait_timeout(), 1)
        self.assertEqual(len(calls), 1)
        self.now = 6
        self.owner.tick()
        self.assertEqual(len(calls), 2)

    def test_fallback_uses_slow_idle_rate_and_faster_open_rate(self):
        def unavailable():
            raise RuntimeError('USB permission missing')
        owner = Watch(self.messages.append, device_factory=unavailable,
                      clock=lambda: self.now, present=lambda: True,
                      fallback=lambda: dict(connected=True, usb=False, gain=43))
        owner.sample()
        self.assertEqual(owner.next_poll, 3)
        self.assertEqual(owner.wait_timeout(), 1)  # liveness without amixer
        owner.command(dict(id=1, op='poll', intervalMs=50))
        self.assertEqual(owner.next_poll, 1.5)

    def test_physical_update_never_writes(self):
        self.device.blocks[4][0] = 46
        self.now += .5
        self.owner.tick()
        self.assertEqual(self.messages[-1]['state']['gain'], 46)
        self.assertEqual(self.device.writes, [])

    def test_set_reads_back_and_preserves_concurrent_other_field(self):
        self.device.concurrent = True
        result = self.command(op='set', key='gain', value=41)
        self.assertTrue(result['ok'])
        self.assertEqual(result['state']['gain'], 41)
        self.assertEqual(result['state']['strength'], 12)
        self.assertEqual(self.messages[-2]['type'], 'state')

    def test_invalid_commands_do_not_write_or_disconnect(self):
        for value in [float('nan'), float('inf'), -1, 81, '12', None]:
            self.assertFalse(self.command(op='set', key='gain', value=value)['ok'])
        self.assertFalse(self.command(op='set', key='nope', value=1)['ok'])
        self.assertFalse(self.command(op='poll', intervalMs=1)['ok'])
        self.assertTrue(self.owner.state['connected'])
        self.assertEqual(self.device.writes, [])

    def test_failed_write_is_not_replayed_on_reconnect(self):
        self.device.fail_write = True
        result = self.command(op='set', key='gain', value=40)
        self.assertFalse(result['ok'])
        self.assertFalse(result['state']['connected'])
        self.assertTrue(result['state']['stale'])
        self.assertEqual(result['state']['gain'], 43)
        self.assertTrue(self.device.closed)
        self.assertFalse(self.command(op='set', key='gain', value=39)['ok'])
        replacement = FakeDevice()
        self.owner.device_factory = lambda: replacement
        self.now += .5
        self.owner.tick()
        self.assertTrue(self.owner.state['connected'])
        self.assertEqual(replacement.writes, [])
        self.assertEqual(self.owner.state['gain'], 43)

    def test_reconnect_backoff_and_stale_heartbeat(self):
        calls = []
        def missing():
            calls.append(self.now)
            raise RuntimeError('disconnected')
        self.owner.close()
        self.owner.device_factory = missing
        self.owner.sample()
        self.assertEqual(self.owner.next_connect, 1.25)
        self.now = 1.1
        self.owner.sample()
        self.assertEqual(len(calls), 1)
        self.now = 2.1
        self.owner.tick()
        self.assertEqual(len(calls), 2)
        self.assertEqual(self.messages[-1]['type'], 'state')
        self.assertTrue(self.messages[-1]['state']['stale'])
        self.assertEqual(self.messages[-1]['sampledAtMs'], 1000)

    def test_fallback_only_permission_failure_and_present_card(self):
        for reason, present, allowed in [('USB interface busy', True, False),
                                        ('USB permission missing', False, False),
                                        ('USB permission missing', True, True)]:
            def unavailable():
                raise RuntimeError(reason)
            owner = Watch(self.messages.append, device_factory=unavailable,
                          clock=lambda: self.now, present=lambda: present,
                          fallback=lambda: dict(connected=True, usb=False, gain=43))
            owner.sample()
            self.assertEqual(owner.state['connected'], allowed)

    def test_bad_device_state_during_write_disables_edits(self):
        self.device.blocks[4][0] = 255
        result = self.command(op='set', key='gain', value=40)
        self.assertFalse(result['ok'])
        self.assertTrue(result['state']['stale'])
        self.assertFalse(result['state']['connected'])
        self.assertEqual(self.device.writes, [])

    def test_permission_retries_do_not_flicker_working_fallback(self):
        def unavailable():
            raise RuntimeError('USB permission missing')
        owner = Watch(self.messages.append, device_factory=unavailable,
                      clock=lambda: self.now, present=lambda: True,
                      fallback=lambda: dict(connected=True, usb=False, gain=43))
        owner.sample()
        self.messages.clear()
        self.now += 1
        owner.tick()
        self.assertTrue(self.messages)
        self.assertTrue(all(m['state']['connected'] for m in self.messages))

    def test_defaults_runs_once_only_when_connected(self):
        calls = []
        self.owner.set_defaults = lambda: calls.append(1)
        self.assertTrue(self.command(op='defaults')['ok'])
        self.owner.failed(RuntimeError('gone'))
        self.assertFalse(self.command(op='defaults')['ok'])
        self.assertEqual(calls, [1])

class FallbackTests(unittest.TestCase):
    def test_three_alsa_values_from_one_process(self):
        fixture = """numid=1,iface=MIXER,name='Elgato Wave XLR MK.2 Capture Volume'
  ; type=INTEGER,access=rw---R--,values=1,min=0,max=80,step=0
  : values=43
numid=2,iface=MIXER,name='Elgato Wave XLR MK.2 Capture Switch'
  : values=off
numid=3,iface=MIXER,name='Elgato Wave XLR MK.2 Playback Volume'
  : values=160,160
"""
        with patch('control.subprocess.check_output', return_value=fixture) as run:
            state = control.alsa()
        self.assertEqual((state['gain'], state['mute'], state['headphones']), (43, True, -20))
        run.assert_called_once_with(['amixer', '-c', 'MK2', 'contents'], text=True, timeout=3)

    def test_library_discovery_is_cached_across_reconnects(self):
        control.usb_library_name.cache_clear()
        with patch('control.ctypes.util.find_library', return_value='libusb-test.so') as find:
            self.assertEqual(control.usb_library_name(), 'libusb-test.so')
            self.assertEqual(control.usb_library_name(), 'libusb-test.so')
            find.assert_called_once()
        control.usb_library_name.cache_clear()

class StreamTests(unittest.TestCase):
    def test_ndjson_invalid_oversized_commands_and_eof_cleanup(self):
        script = """
import control
from test_watch import FakeDevice
original = control.Watch
device = FakeDevice()
control.Watch = lambda emit: original(emit, device_factory=lambda: device)
control.watch()
assert device.closed
"""
        payload = (b'{broken}\n' + b'x' * 5000 + b'\n' +
                   b'{"id":1,"op":"poll","intervalMs":50}\n' +
                   b'{"id":2,"op":"set","key":"gain","value":NaN}\n')
        result = subprocess.run([sys.executable, '-c', script],
                                cwd=Path(__file__).parent, input=payload,
                                capture_output=True, timeout=3)
        self.assertEqual(result.returncode, 0, result.stderr)
        events = [json.loads(line) for line in result.stdout.splitlines()]
        results = [m for m in events if m['type'] == 'result']
        self.assertEqual(len(results), 4)
        self.assertFalse(results[0]['ok'])
        self.assertIn('4096', results[1]['error'])
        self.assertTrue(results[2]['ok'])
        self.assertFalse(results[3]['ok'])

    def test_existing_owner_fails_fast_without_attempting_usb(self):
        with tempfile.TemporaryDirectory() as runtime:
            with open(runtime + '/wave-xlr-control.lock', 'a') as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                result = subprocess.run([sys.executable, str(Path(__file__).with_name('control.py'))],
                                        env=dict(os.environ, XDG_RUNTIME_DIR=runtime),
                                        capture_output=True, text=True, timeout=2)
                self.assertEqual(result.returncode, 1)
                self.assertIn('owner already running', json.loads(result.stdout)['error'])

if __name__ == '__main__':
    unittest.main()
