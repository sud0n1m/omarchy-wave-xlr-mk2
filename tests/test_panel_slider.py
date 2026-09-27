#!/usr/bin/env python3
"""Exercise actual live SliderField in isolated offscreen Quickshell; never opens USB.

Run: python3 tests/test_panel_slider.py
QtTest is loaded inside Quickshell because its modules are statically linked.
The live component source is extracted every run, so this guards actual handlers.
"""
from pathlib import Path
import os
import re
import subprocess
import sys
import tempfile

HEADER = 'import Quickshell\nimport QtQuick\nimport QtTest\nimport qs.Ui\nimport qs.Commons\nFloatingWindow {\n id: root; visible:true; implicitWidth: 500; implicitHeight: 200\n property color fg: Color.foreground\n property string family: Style.font.family\n property var bar: null\n property var state: ({headphones:-20})\n property var highlights: ({})\n property bool opened: true\n property bool available: true\n property string focusedControl: ""\n property int edits: 0\n property var calls: []\n function value(k) { return state[k] }\n function supported(k) { return available }\n function ensureVisible(i) {}\n function setDragging(k,d) {}\n function edit(k,v,final) { edits++; calls.push({value:v,final:final}); var n={}; n[k]=v; state=n }\n'
TESTS = ' SliderField { id: field; width: 400; controlKey: "headphones"; minimum: -60; maximum: 0; increment: .25 }\n TestCase {\n  name: "WaveActualSliderField"; when: true\n  function initTestCase() { console.log("WAVE TESTS START") }\n  function cleanupTestCase() { console.log("WAVE TESTS COMPLETE", qtest_results.passCount, qtest_results.failCount) }\n  function cleanup() { console.log("WAVE CASE", qtest_results.functionName, "edits",root.edits,"value",root.state.headphones,"failures", qtest_results.failCount) }\n  function slider() { return findChild(field, "headphones") }\n  function init() { root.state = ({headphones:-20}); root.edits=0; root.calls=[]; root.available=true; root.opened=true; wait(20) }\n  function test_wheelSingleDelivery() { var item=slider(); verify(item); mouseWheel(item,item.width/2,item.height/2,0,120); compare(root.state.headphones,-19); compare(root.edits,1) }\n  function test_shiftWheelSingleDelivery() { var item=slider(); mouseWheel(item,item.width/2,item.height/2,0,-120,Qt.NoButton,Qt.ShiftModifier); compare(root.state.headphones,-20.25); compare(root.edits,1) }\n  function test_disabledWheel() { root.available=false; var item=slider(); mouseWheel(item,item.width/2,item.height/2,0,120); compare(root.state.headphones,-20); compare(root.edits,0) }\n  function test_dragQuarterSnapFinal() { var item=slider(); mousePress(item,101,item.height/2); mouseMove(item,213,item.height/2,20); mouseRelease(item,213,item.height/2); compare(root.state.headphones*4,Math.round(root.state.headphones*4)); verify(root.calls.length>=2); verify(root.calls[root.calls.length-1].final); compare(item.dragging,false) }\n  function test_dragReadNoYank() { var item=slider(); mousePress(item,101,item.height/2); var local=item.liveValue; root.state=({headphones:-5}); wait(20); compare(item.liveValue,local); mouseRelease(item,101,item.height/2); compare(root.state.headphones,field.snap(local)); verify(root.calls[root.calls.length-1].final) }\n  function test_externalReadNoWrite() { root.state=({headphones:-23.5}); wait(30); compare(slider().value,-23.5); compare(root.edits,0) }\n  function test_closeCancelsDrag() { var item=slider(); mousePress(item,101,item.height/2); verify(item.dragging); root.opened=false; compare(item.dragging,false); mouseRelease(item,101,item.height/2) }\n }\n}\n'


def main():
    panel = Path(__file__).resolve().parents[1] / "Panel.qml"
    source = panel.read_text()
    components = source[source.index("    component Label:"):source.index("    component Effect:")]
    with tempfile.TemporaryDirectory(prefix="wave-slider-test-") as tmp:
        directory = Path(tmp)
        for module in ("Commons", "Ui"):
            (directory / module).symlink_to(Path("/usr/share/omarchy/shell") / module)
        (directory / "shell.qml").write_text(HEADER + components + TESTS)
        env = dict(os.environ, QT_QPA_PLATFORM="offscreen")
        result = subprocess.run(["quickshell", "--no-color", "-p", str(directory / "shell.qml")],
                                env=env, text=True, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, timeout=20)
        for line in result.stdout.splitlines():
            if "WAVE " in line or "ERROR" in line:
                print(line)
        counts = re.search(r"WAVE TESTS COMPLETE (\d+) (\d+)", result.stdout)
        if not counts:
            print(result.stdout)
            return 1
        return int(result.returncode != 0 or int(counts[2]) != 0)


if __name__ == "__main__":
    sys.exit(main())
