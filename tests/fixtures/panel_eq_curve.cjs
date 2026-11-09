// Check the EQ graph's response math, extracted from the actual panel, against known filter behavior.
const {readFileSync} = require('node:fs');
const {join} = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const source = readFileSync(join(__dirname, '..', '..', 'Panel.qml'), 'utf8');
const names = ['eqSignature', 'eqBands', 'eqBiquad', 'eqDb', 'eqX', 'eqY', 'eqCurve'];
const functions = names.map(name => {
    const start = source.indexOf('    function ' + name + '(');
    const end = source.indexOf('\n    function ', start + 1);
    if (start < 0 || end < 0) throw new Error('Cannot extract panel function: ' + name);
    return source.slice(start, end);
}).join('\n');

const ctx = {Math, Number, Qt: {point: (x, y) => ({x, y})}, state: {}, value: key => ctx.state[key]};
vm.createContext(ctx);
vm.runInContext(functions, ctx);

const flat = {eq: true};
for (let n = 1; n <= 4; n++) Object.assign(flat, {[`eq${n}On`]: true, [`eq${n}Type`]: 2, [`eq${n}Freq`]: 1000, [`eq${n}Gain`]: 0, [`eq${n}Q`]: 1});
const db = (bands, freq) => ctx.eqDb(freq, bands.map(ctx.eqBiquad));
const near = (actual, expected, tolerance, message) =>
    assert.ok(Math.abs(actual - expected) <= tolerance, `${message}: ${actual} not within ${tolerance} of ${expected}`);

ctx.state = {...flat};
const signature = ctx.eqSignature();
assert.equal(signature, '1|1,2,1000,0,1|1,2,1000,0,1|1,2,1000,0,1|1,2,1000,0,1');
for (const f of [20, 100, 1000, 10000, 20000]) near(db(ctx.eqBands(signature), f), 0, 1e-9, `flat bells at ${f} Hz`);

near(db([{type: 2, freq: 1000, gain: 6, q: 1}], 1000), 6, 1e-6, 'bell peaks at its gain');
near(db([{type: 2, freq: 1000, gain: -6, q: 1}], 1000), -6, 1e-6, 'bell cuts at its gain');
near(db([{type: 0, freq: 100, gain: 0, q: Math.SQRT1_2}], 100), -3.01, 0.01, 'Butterworth high-pass is -3 dB at cutoff');
near(db([{type: 0, freq: 100, gain: 0, q: Math.SQRT1_2}], 10000), 0, 0.01, 'high-pass passes highs');
assert.ok(db([{type: 0, freq: 100, gain: 0, q: Math.SQRT1_2}], 25) < -20, 'high-pass rejects lows');
near(db([{type: 5, freq: 1000, gain: 0, q: Math.SQRT1_2}], 1000), -3.01, 0.01, 'low-pass is -3 dB at cutoff');
near(db([{type: 1, freq: 200, gain: 6, q: Math.SQRT1_2}], 20), 6, 0.05, 'low shelf reaches its gain');
near(db([{type: 1, freq: 200, gain: 6, q: Math.SQRT1_2}], 20000), 0, 0.05, 'low shelf leaves highs');
near(db([{type: 4, freq: 5000, gain: -4, q: Math.SQRT1_2}], 20000), -4, 0.3, 'high shelf reaches its gain');
assert.ok(db([{type: 3, freq: 1000, gain: 0, q: 2}], 1000) < -40, 'notch removes its frequency');
near(db([{type: 2, freq: 1000, gain: 0, q: 1}, {type: 2, freq: 1000, gain: 3, q: 1}, {type: 2, freq: 1000, gain: 3, q: 1}], 1000), 6, 1e-6, 'bands sum in dB');

// Gain-less types ignore a stored gain, as the device does.
near(db([{type: 0, freq: 100, gain: 12, q: Math.SQRT1_2}], 10000), 0, 0.01, 'high-pass ignores gain');

ctx.state = {...flat, eq2On: false, eq2Gain: 12};
assert.equal(ctx.eqCurve(ctx.eqSignature(), 300, 100, 12, false).every(p => p.y === 50), true, 'disabled bands are left out');
const curve = ctx.eqCurve(ctx.eqSignature(), 300, 100, 12, false);
assert.equal(curve.length, 97);
assert.equal(curve[0].x + ','  + curve[96].x, '0,300', 'curve spans 20 Hz to 20 kHz');
assert.equal(ctx.eqCurve(ctx.eqSignature(), 300, 100, 12, true).length, 100, 'closed curve returns along the 0 dB line');
assert.equal(ctx.eqY(40, 100, 12), 0, 'gain beyond the range clamps to the edge');
ctx.state = {gain: 40};
assert.equal(ctx.eqSignature(), '', 'no EQ, no curve');
assert.equal(ctx.eqCurve('', 300, 100, 12, true).length, 0);
console.log('PASS EQ curve math');
