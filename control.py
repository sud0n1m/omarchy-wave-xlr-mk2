#!/usr/bin/env python3
"""Wave XLR MK.2 control. Protocol reference: emaspa/openxlr (see README)."""
import ctypes as C
import ctypes.util
import fcntl
import json
import os
import re
import subprocess
import sys
import math
import selectors
import time
from functools import cache

LENGTHS = {1: 6, 4: 38, 5: 2}
FLAGS = {'mute': 1, 'phantom': 2, 'lowCut': 16, 'expander': 32, 'voiceTune': 64, 'compressor': 128}

def modify(blocks, key, value):
    b = {k: bytearray(v) for k, v in blocks.items()}
    if key in FLAGS or key in ('clipguard', 'lowImpedance'):
        if value not in (0, 1): raise ValueError('Expected 0 or 1')
        block, offset, mask = (4, 1, FLAGS[key]) if key in FLAGS else ((4, 2, 4) if key == 'clipguard' else (5, 1, 2))
        on = not value if key == 'clipguard' else value
        b[block][offset] = (b[block][offset] | mask) if on else (b[block][offset] & ~mask)
    else:
        ranges = {'gain': (4, 0, 0, 80), 'headphones': (5, 0, -60, 0), 'balance': (1, 0, 0, 200), 'strength': (4, 10, 0, 100)}
        if key not in ranges: raise ValueError('Unknown control')
        block, offset, lo, hi = ranges[key]
        if not lo <= value <= hi: raise ValueError('Value out of range')
        b[block][offset] = round(-value * 4) if key == 'headphones' else int(value)
    return block, b[block]

def decode(b):
    s, h, x = b[4], b[5], b[1]
    if s[0] > 80 or h[0] > 240 or x[0] > 200 or s[10] > 100: raise ValueError('Unexpected device state; controls disabled')
    return dict(connected=True, usb=True, gain=s[0], headphones=-h[0]/4, balance=x[0], strength=s[10], clipguard=not bool(s[2]&4), lowImpedance=bool(h[1]&2), **{k: bool(s[1]&v) for k,v in FLAGS.items()})

@cache
def usb_library_name():
    # find_library can spawn ldconfig; resolve once, including during reconnects.
    return ctypes.util.find_library("usb-1.0") or "libusb-1.0.so.0"


class Device:
    def __init__(self):
        self.lib = C.CDLL(usb_library_name())
        api = {'libusb_init': ([C.POINTER(C.c_void_p)], C.c_int), 'libusb_open_device_with_vid_pid': ([C.c_void_p,C.c_ushort,C.c_ushort],C.c_void_p), 'libusb_claim_interface': ([C.c_void_p,C.c_int],C.c_int), 'libusb_release_interface': ([C.c_void_p,C.c_int],C.c_int), 'libusb_close': ([C.c_void_p],None), 'libusb_exit': ([C.c_void_p],None), 'libusb_control_transfer': ([C.c_void_p,C.c_ubyte,C.c_ubyte,C.c_ushort,C.c_ushort,C.c_void_p,C.c_ushort,C.c_uint],C.c_int)}
        for name,(args,rest) in api.items():
            f=getattr(self.lib,name); f.argtypes=args; f.restype=rest
        self.ctx=C.c_void_p(); self.handle=None; self.claimed=False
        if self.lib.libusb_init(C.byref(self.ctx)) != 0: raise RuntimeError('USB initialization failed')
        self.handle=self.lib.libusb_open_device_with_vid_pid(self.ctx,0x0fd9,0x00b6)
        if not self.handle: self.close(); raise RuntimeError('Wave XLR MK.2 disconnected or USB permission missing')
        code=self.lib.libusb_claim_interface(self.handle,3)
        if code != 0: self.close(); raise RuntimeError(f'USB control interface busy or inaccessible ({code})')
        self.claimed=True
    def transfer(self, block, data=None):
        n=LENGTHS[block]; buf=(C.c_ubyte*n)(*(data or bytes(n)))
        result=self.lib.libusb_control_transfer(self.handle,0x41 if data is not None else 0xc1,1,block,0x0203,buf,n,1000)
        if result != n: raise RuntimeError(f'USB block {block}: received {result}, expected {n}; no further writes')
        return bytearray(buf)
    def read(self): return {k:self.transfer(k) for k in LENGTHS}
    def close(self):
        if self.handle:
            if self.claimed: self.lib.libusb_release_interface(self.handle,3)
            self.lib.libusb_close(self.handle); self.handle=None
        if self.ctx: self.lib.libusb_exit(self.ctx); self.ctx=None

def defaults():
    nodes=json.loads(subprocess.check_output(['pw-dump'],timeout=4))
    found={}
    for n in nodes:
        p=n.get('info',{}).get('props',{})
        if 'Elgato_Wave_XLR_MK.2' in p.get('node.name','') and p.get('media.class') in ('Audio/Sink','Audio/Source'):
            found[p['media.class']]=str(n['id'])
    if len(found)!=2: raise RuntimeError('Wave input/output not both available')
    for node in found.values(): subprocess.run(['wpctl','set-default',node],check=True,timeout=3)

def alsa(key=None, value=None):
    controls = {'gain': 'Elgato Wave XLR MK.2 Capture Volume', 'mute': 'Elgato Wave XLR MK.2 Capture Switch', 'headphones': 'Elgato Wave XLR MK.2 Playback Volume'}
    if key == 'defaults': defaults()
    elif key:
        if key not in controls: raise RuntimeError('USB permission needed for DSP controls')
        modify({k:bytes(v) for k,v in LENGTHS.items()},key,value)  # range validation
        raw = ('off' if value else 'on') if key=='mute' else str(round(240+value*4) if key=='headphones' else int(value))
        subprocess.run(['amixer','-q','-c','MK2','cset','name='+controls[key],raw],check=True,timeout=3)
    text = subprocess.check_output(['amixer', '-c', 'MK2', 'contents'], text=True, timeout=3)
    values = {}
    for block in re.split(r'(?=^numid=)', text, flags=re.M):
        name = re.search(r"name='([^']+)'", block)
        value = re.search(r'^\s*: values=(\w+)', block, re.M)
        if name and value:
            values[name.group(1)] = value.group(1)
    return dict(connected=True, usb=False, gain=int(values[controls['gain']]),
                mute=values[controls['mute']]=='off',
                headphones=(int(values[controls['headphones']])-240)/4,
                note='Basic controls available · DSP needs USB access')


class WriteRejected(ValueError):
    """A valid refreshed state did not match the requested field."""


class Watch:
    """Single serialized USB owner. Commands are never retained across failures."""
    def __init__(self, emit, device_factory=Device, clock=time.monotonic,
                 fallback=alsa, present=None, set_defaults=defaults):
        self.emit = emit
        self.device_factory = device_factory
        self.clock = clock
        self.fallback = fallback
        self.present = present or self.alsa_present
        self.set_defaults = set_defaults
        self.device = None
        self.fallback_allowed = False
        self.state = {'connected': False, 'usb': False, 'stale': True}
        self.interval = .5
        self.next_poll = 0
        self.next_connect = 0
        self.backoff = .25
        self.last_emit = -float('inf')
        self.sampled_at = 0
        self.seq = 0

    @staticmethod
    def alsa_present():
        try:
            with open('/proc/asound/cards') as cards:
                return bool(re.search(r'\[MK2\s*\]', cards.read()))
        except OSError:
            return False

    def close(self):
        if self.device:
            self.device.close()
            self.device = None

    def publish(self, state, force=False, sampled=True):
        now = self.clock()
        if sampled:
            self.sampled_at = int(now * 1000)
        changed = state != self.state
        self.state = state
        if changed or force or now - self.last_emit >= 1:
            self.seq += 1
            self.emit({'type': 'state', 'state': state, 'seq': self.seq,
                       'sampledAtMs': self.sampled_at})
            self.last_emit = now

    def failed(self, error):
        self.close()
        self.next_connect = self.clock() + self.backoff
        self.backoff = min(5, self.backoff * 2)
        self.publish(dict(self.state, connected=False, usb=False, stale=True,
                          error=str(error)), sampled=False)

    def sample(self, force=False):
        now = self.clock()
        if not self.device and now >= self.next_connect:
            try:
                self.device = self.device_factory()
                self.fallback_allowed = False
            except Exception as error:
                self.fallback_allowed = 'permission missing' in str(error)
                if self.fallback_allowed and self.present() and self.state.get('connected') and not self.state.get('usb'):
                    # A failed vendor retry must not flicker working ALSA controls.
                    self.next_connect = now + self.backoff
                    self.backoff = min(5, self.backoff * 2)
                else:
                    self.failed(error)
        try:
            if self.device:
                state = decode(self.device.read())
                self.backoff = .25
            elif self.fallback_allowed and self.present():
                state = self.fallback()
            else:
                self.publish(self.state, force=force, sampled=False)
                return
            self.publish(dict(state, stale=False), force=force)
        except Exception as error:
            self.failed(error)
        finally:
            # Don't repeatedly probe a missing card between reconnect attempts.
            # Permission-only fallback uses one amixer, 2 Hz open / 0.5 Hz closed.
            now = self.clock()
            if self.device:
                self.next_poll = now + self.interval
            elif self.state.get('connected') and not self.state.get('stale'):
                self.next_poll = now + (.5 if self.interval == .05 else 2)
            else:
                self.next_poll = max(now + .05, self.next_connect)

    def tick(self):
        if self.clock() >= self.next_poll:
            self.sample()
        elif self.clock() - self.last_emit >= 1:
            self.publish(self.state, sampled=False)

    def wait_timeout(self):
        # Stdin readiness wakes immediately; no 100 ms idle polling needed.
        return max(0, min(self.next_poll, self.last_emit + 1) - self.clock())

    def command(self, message):
        identifier = message.get('id') if isinstance(message, dict) else None
        try:
            if not isinstance(message, dict):
                raise ValueError('Expected a JSON command object')
            if not isinstance(identifier, (str, int)) or isinstance(identifier, bool):
                raise ValueError('Command id must be a string or integer')
            op = message.get('op')
            if op == 'poll':
                interval = message.get('intervalMs')
                if isinstance(interval, bool) or interval not in (50, 500):
                    raise ValueError('intervalMs must be 50 (open) or 500 (closed)')
                self.interval = interval / 1000
                self.sample(force=True)
            elif op == 'refresh':
                self.sample(force=True)
            elif op in ('set', 'defaults'):
                if not self.state.get('connected') or self.state.get('stale'):
                    raise RuntimeError('Device is disconnected; refresh after reconnection before editing')
                if op == 'defaults':
                    self.set_defaults()
                else:
                    key, value = message.get('key'), message.get('value')
                    if not isinstance(key, str):
                        raise ValueError('Control key must be a string')
                    if not isinstance(value, (int, float)) or not math.isfinite(value):
                        raise ValueError('Control value must be a finite number')
                    # Validate without USB access; invalid requests do not disconnect.
                    modify({k: bytes(n) for k, n in LENGTHS.items()}, key, value)
                    if not self.device and key not in ('gain', 'mute', 'headphones'):
                        raise ValueError('USB permission needed for DSP controls')
                    try:
                        if self.device:
                            before = self.device.read()
                            decode(before)
                            block, data = modify(before, key, value)
                            self.device.transfer(block, data)
                            after = self.device.read()
                            expected = dict(before)
                            expected[block] = data
                            if decode(after)[key] != decode(expected)[key]:
                                self.publish(dict(decode(after), stale=False), force=True)
                                raise WriteRejected('Device did not confirm the requested setting')
                            self.publish(dict(decode(after), stale=False), force=True)
                        else:
                            self.publish(dict(self.fallback(key, value), stale=False), force=True)
                    except WriteRejected:
                        raise
                    except Exception as error:
                        self.failed(error)
                        raise
            else:
                raise ValueError('Unknown operation')
            self.emit({'type': 'result', 'id': identifier, 'ok': True, 'state': self.state})
        except Exception as error:
            self.emit({'type': 'result', 'id': identifier, 'ok': False,
                       'error': str(error), 'state': self.state})


def watch():
    def emit(message):
        print(json.dumps(message, allow_nan=False), flush=True)
    owner = Watch(emit)
    selector = selectors.DefaultSelector()
    selector.register(sys.stdin, selectors.EVENT_READ)
    pending = bytearray()
    dropping = False
    try:
        owner.sample(force=True)
        while True:
            owner.tick()
            ready = selector.select(owner.wait_timeout())
            if not ready:
                continue
            chunk = os.read(sys.stdin.fileno(), 4096)
            if not chunk:
                return  # EOF means the shell stopped; release USB immediately.
            for byte in chunk:
                if byte == 10:
                    if not dropping and pending:
                        try:
                            message = json.loads(pending)
                            owner.command(message)
                        except (ValueError, UnicodeDecodeError) as error:
                            emit({'type': 'result', 'id': None, 'ok': False,
                                  'error': 'Invalid JSON command: ' + str(error)})
                    pending.clear()
                    dropping = False
                    owner.tick()
                elif not dropping:
                    pending.append(byte)
                    if len(pending) > 4096:
                        emit({'type': 'result', 'id': None, 'ok': False,
                              'error': 'Command exceeds 4096 bytes'})
                        pending.clear()
                        dropping = True
    finally:
        selector.close()
        owner.close()


def main():
    runtime=os.environ.get('XDG_RUNTIME_DIR',f'/run/user/{os.getuid()}')
    with open(runtime+'/wave-xlr-control.lock','a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RuntimeError('Wave XLR owner already running; send commands through its NDJSON stdin')
        if sys.argv[1:] == ['--watch']:
            watch()
            return
        try: d=Device()
        except RuntimeError as e:
            if 'permission missing' not in str(e): raise
            print(json.dumps(alsa(sys.argv[1] if len(sys.argv)>1 else None, float(sys.argv[2]) if len(sys.argv)>2 else None)))
            return
        try:
            before=d.read(); state=decode(before)
            if len(sys.argv)>1:
                if sys.argv[1]=='defaults': defaults()
                else:
                    key=sys.argv[1]; value=float(sys.argv[2]); block,data=modify(before,key,value)
                    d.transfer(block,data)
                    after=d.read()
                    if after[block] != data: raise RuntimeError('Device did not confirm the requested setting; refreshed state required')
                    state=decode(after)
            print(json.dumps(state))
        finally: d.close()
if __name__=='__main__':
    try: main()
    except BrokenPipeError: sys.exit(0)
    except Exception as e: print(json.dumps({'connected':False,'error':str(e)})); sys.exit(1)
