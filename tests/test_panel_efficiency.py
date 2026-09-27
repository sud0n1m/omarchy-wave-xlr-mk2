"""Exercise actual panel functions in Node's JS VM; no shell or USB access."""
from pathlib import Path
import subprocess

panel = Path(__file__).resolve().parents[1] / 'Panel.qml'
source = panel.read_text()
def function(name):
    start = source.index('    function ' + name + '(')
    end = source.index('\n    function ', start + 1)
    return source[start:end]

script = """
const vm = require('node:vm');
const assert = require('node:assert/strict');
const ctx = {Date, Number, opened:false, state:{connected:true,usb:true,stale:false,gain:43},
 desired:{}, queued:{}, pending:{}, dragging:{}, highlights:{},
 stale:false, transportError:'', supported:()=>true,
 flushCount:0, watchdogCount:0, coalesce:{running:false,start(){}},
 copy:o=>({...o}), flush:()=>ctx.flushCount++, armWatchdog:()=>ctx.watchdogCount++};
ctx.root=ctx;
ctx.value=key=>ctx.desired[key] ?? ctx.state[key];
vm.createContext(ctx);
""" + function('edit') + '\n' + function('acceptState') + """
vm.runInContext(edit.toString()+';'+acceptState.toString(),ctx);
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
assert.equal(ctx.watchdogCount,1,'heartbeat must reset watchdog');
ctx.acceptState({...ctx.state,gain:45});
assert.equal(ctx.state.gain,45,'changed device values must still be delivered');
console.log('PASS unchanged edits, final flush, heartbeat identity and changed state');
"""
subprocess.run(['node', '-e', script], check=True)
