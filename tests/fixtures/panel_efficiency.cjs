// Exercise functions extracted from the actual panel; no shell or USB access.
const {readFileSync} = require('node:fs');
const {join} = require('node:path');
const source = readFileSync(join(__dirname, '..', '..', 'Panel.qml'), 'utf8');
const names = ['copy', 'send', 'edit', 'armWatchdog', 'syncWatchdog', 'acceptState', 'receive'];
const functions = names.map(name => {
    const start = source.indexOf('    function ' + name + '(');
    const end = source.indexOf('\n    function ', start + 1);
    if (start < 0 || end < 0) throw new Error('Cannot extract panel function: ' + name);
    return source.slice(start, end);
}).join('\n');

const vm = require('node:vm');
const assert = require('node:assert/strict');
const ctx = {Date, Number, opened:false, state:{connected:true,usb:true,stale:false,gain:43},
 desired:{}, queued:{}, pending:{}, requests:{}, dragging:{}, highlights:{},
 stale:false, workerQuiet:false, transportError:'', error:'', supported:()=>true,
 serial:0,sequence:-1,defaultsId:-1,defaultsDeadline:0,
 flushCount:0, watchdogCount:0, coalesce:{running:false,start(){}},
 flush:()=>ctx.flushCount++, writes:[]};
ctx.watchdog={running:false,interval:3100,restart:()=>{ctx.watchdogCount++;ctx.watchdog.running=true},stop:()=>ctx.watchdog.running=false};
ctx.worker={running:true,write:line=>ctx.writes.push(JSON.parse(line))};
ctx.root=ctx;
ctx.value=key=>ctx.desired[key] ?? ctx.state[key];
vm.createContext(ctx);
vm.runInContext(functions,ctx);
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
{
  const before=ctx.state, beforeHighlights=ctx.highlights, beforeWatchdog=ctx.watchdogCount, seq=ctx.sequence;
  ctx.lastSeen=0;
  ctx.receive(JSON.stringify({type:'state',seq:seq+1,sampledAtMs:1}));
  assert.equal(ctx.state,before,'liveness-only heartbeat keeps state identity');
  assert.equal(ctx.highlights,beforeHighlights,'liveness-only heartbeat keeps highlights');
  assert.equal(ctx.watchdogCount,beforeWatchdog+1,'liveness-only heartbeat resets watchdog');
  assert.ok(ctx.lastSeen>0,'liveness-only heartbeat refreshes liveness');
  assert.equal(ctx.error,'','liveness-only heartbeat is not an unreadable response');
  ctx.receive(JSON.stringify({type:'state',seq:seq+1,sampledAtMs:1}));
  assert.equal(ctx.watchdogCount,beforeWatchdog+1,'replayed heartbeat is ignored');
  ctx.receive(JSON.stringify({type:'state',seq:seq+2,sampledAtMs:2,patch:{gain:46}}));
  assert.equal(ctx.state.gain,46,'patch frames update the changed field');
  assert.equal(ctx.state.usb,true,'patch frames keep the other fields');
  assert.notEqual(ctx.state,before,'a real change replaces the state object for bindings');
  const patched=ctx.state;
  ctx.receive(JSON.stringify({type:'state',seq:seq+3,sampledAtMs:3,patch:{gain:46}}));
  assert.equal(ctx.state,patched,'a no-op patch keeps state identity');
  ctx.receive(JSON.stringify({type:'result',id:12345,ok:true}));
  assert.equal(ctx.state,patched,'a stateless result keeps state');
  assert.equal(ctx.error,'','a stateless result is not an error');
  ctx.receive(JSON.stringify({type:'state',seq:seq+4,sampledAtMs:4,patch:{gain:45}}));
}
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
{
  // Closing the panel: the worker's frame says it is quiet; once the poll is
  // acknowledged nothing is outstanding and the shell stops expecting heartbeats.
  ctx.opened=false;
  const close=ctx.send('poll',{intervalMs:500});
  const seq=ctx.sequence;
  ctx.receive(JSON.stringify({type:'state',seq:seq+1,sampledAtMs:5,quiet:true,state:ctx.state}));
  assert.equal(ctx.workerQuiet,true);
  assert.equal(ctx.watchdog.running,true,'the close poll is still outstanding');
  ctx.receive(JSON.stringify({type:'result',id:close,ok:true}));
  assert.equal(ctx.watchdog.running,false,'a quiet worker with the panel closed is not timed');
  ctx.receive(JSON.stringify({type:'state',seq:seq+2,sampledAtMs:6,quiet:true,patch:{gain:47}}));
  assert.equal(ctx.state.gain,47,'quiet frames still deliver changes');
  assert.equal(ctx.watchdog.running,false);
  // Any frame without the flag (lost events, a failed read) re-arms it.
  ctx.receive(JSON.stringify({type:'state',seq:seq+3,sampledAtMs:6}));
  assert.equal(ctx.workerQuiet,false);
  assert.equal(ctx.watchdog.running,true,'heartbeats are expected again');
  // A quiet flag never disarms an open panel or a stale device.
  ctx.receive(JSON.stringify({type:'state',seq:seq+4,sampledAtMs:7,quiet:true}));
  assert.equal(ctx.watchdog.running,false);
  ctx.opened=true;
  ctx.send('poll',{intervalMs:50});
  assert.equal(ctx.watchdog.running,true,'opening times the poll');
  ctx.receive(JSON.stringify({type:'state',seq:seq+5,sampledAtMs:8,quiet:true}));
  assert.equal(ctx.watchdog.running,true,'an open panel is always timed');
}
console.log('PASS state identity, edit coalescing, event-only absence, request timeouts, wake and quiet watchdog');
