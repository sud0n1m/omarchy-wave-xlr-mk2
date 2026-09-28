"""Exercise actual panel functions in Node's JS VM; no shell or USB access."""
from pathlib import Path
import json
import subprocess

source = (Path(__file__).resolve().parents[1] / 'Panel.qml').read_text()
def function(name):
    start = source.index('    function ' + name + '(')
    end = source.index('\n    function ', start + 1)
    return source[start:end]

functions = '\n'.join(function(name) for name in ['copy', 'send', 'edit', 'armWatchdog', 'syncWatchdog', 'acceptState', 'receive'])
script = """
const vm = require('node:vm');
const assert = require('node:assert/strict');
const ctx = {Date, Number, opened:false, state:{connected:true,usb:true,stale:false,gain:43},
 desired:{}, queued:{}, pending:{}, requests:{}, dragging:{}, highlights:{},
 stale:false, transportError:'', error:'', supported:()=>true,
 serial:0,sequence:-1,defaultsId:-1,defaultsDeadline:0,
 flushCount:0, watchdogCount:0, coalesce:{running:false,start(){}},
 flush:()=>ctx.flushCount++, writes:[]};
ctx.watchdog={running:false,interval:3100,restart:()=>{ctx.watchdogCount++;ctx.watchdog.running=true},stop:()=>ctx.watchdog.running=false};
ctx.worker={running:true,write:line=>ctx.writes.push(JSON.parse(line))};
ctx.root=ctx;
ctx.value=key=>ctx.desired[key] ?? ctx.state[key];
vm.createContext(ctx);
""" + 'vm.runInContext(' + json.dumps(functions) + ',ctx);\n' + """
ctx.edit('gain',43,false);
assert.equal(ctx.flushCount,0);
assert.deepEqual(ctx.queued,{});
ctx.desired={gain:44};ctx.queued={gain:44};
ctx.edit('gain',44,true);
assert.equal(ctx.flushCount,1,'release still flushes an unacknowledged queued value');
const original=ctx.state, highlights=ctx.highlights;
ctx.acceptState({...ctx.state});
assert.equal(ctx.state,original,'heartbeat must preserve state identity');
assert.equal(ctx.highlights,highlights,'heartbeat must not invalidate highlights');
assert.equal(ctx.watchdogCount,1,'connected heartbeat must reset watchdog');
ctx.acceptState({...ctx.state,gain:45});
assert.equal(ctx.state.gain,45,'changed device values must still be delivered');
ctx.transportError='Device worker stopped'; ctx.error='Old error';
const absent={connected:false,usb:false,stale:false,present:false};
ctx.acceptState(absent);
assert.equal(ctx.stale,false,'known absence is not stale/reconnecting');
assert.equal(ctx.watchdog.running,false,'known absence stops the watchdog');
assert.equal(ctx.transportError,'');assert.equal(ctx.error,'');
assert.equal(ctx.page,'controls');assert.equal(Object.keys(ctx.desired).length,0);
assert.equal(ctx.state.gain,undefined,'absence clears old settings');
const id=ctx.send('poll',{intervalMs:50});
assert.equal(ctx.watchdog.running,true,'explicit commands must be timed even while absent');
assert.equal(ctx.writes.length,1);
ctx.acceptState(absent);
assert.equal(ctx.watchdog.running,true,'state is not a command acknowledgment');
ctx.receive(JSON.stringify({type:'result',id:999,ok:true,state:absent}));
assert.equal(ctx.watchdog.running,true,'unknown acknowledgments must not cancel pending request timeout');
ctx.receive(JSON.stringify({type:'result',id,ok:true,state:absent}));
assert.equal(ctx.watchdog.running,false,'acknowledged command restores indefinite sleep');
assert.equal(Object.keys(ctx.requests).length,0);
ctx.acceptState({connected:false,usb:false,stale:true,present:true});
assert.equal(ctx.watchdog.running,true,'wake notice must arm watchdog before USB work');
ctx.acceptState({connected:true,usb:true,stale:false,present:true,gain:43});
assert.equal(ctx.watchdog.running,true);
const defaults=ctx.send('defaults');
assert.ok(ctx.watchdog.interval>=12000,'explicit defaults command retains longer timeout');
ctx.receive(JSON.stringify({type:'result',id:defaults,ok:true,state:ctx.state}));
assert.equal(ctx.watchdog.interval,3100);
console.log('PASS state identity, edit coalescing, event-only absence, request timeouts and wake watchdog');
"""
subprocess.run(['node', '-e', script], check=True)
