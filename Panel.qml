import QtQuick
import QtQuick.Controls as Controls
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Ui

Panel {
    id: root
    moduleName: "sudonim.wave-xlr"
    ipcTarget: "sudonim.wave-xlr"
    property var state: ({connected:false})
    property string error: ""
    property string transportError: ""
    property double defaultsDeadline: 0
    property int defaultsId: -1
    property string page: "controls"
    property string focusedControl: "panel"
    property bool phantomArmed: false
    property bool phantomTarget: false
    property bool helpOpen: false
    property double lastSeen: 0
    property bool stale: true
    property int serial: 0
    property int sequence: -1
    property var pending: ({})
    property var requests: ({})
    property var desired: ({})
    property var queued: ({})
    property var dragging: ({})
    property var highlights: ({})
    readonly property color fg: Color.popups.text
    readonly property string family: Style.font.family
    readonly property bool disconnected: state.present === false
    readonly property bool available: state.connected === true && !stale && worker.running
    implicitWidth: button.implicitWidth
    implicitHeight: button.implicitHeight
    function copy(obj) { var out = {}; for (var k in obj) out[k] = obj[k]; return out }
    function value(key) { return desired[key] !== undefined ? desired[key] : state[key] }
    function supported(key) { return available && (state.usb || key === "gain" || key === "headphones" || key === "mute") && state[key] !== undefined }
    function send(op, extra) {
        if (!worker.running) return -1
        var cmd = extra || {}; cmd.op = op; cmd.id = ++serial
        if (op === "defaults") { defaultsDeadline = Date.now() + 12000; defaultsId = cmd.id }
        var r = copy(requests); r[cmd.id] = true; requests = r
        armWatchdog()
        worker.write(JSON.stringify(cmd) + "\n")
        return cmd.id
    }
    function edit(key, value, final) {
        if (!supported(key)) return
        if (Number(root.value(key)) === Number(value)) {
            if (final) flush() // Deliver a queued final value, never repeat an acknowledged one.
            return
        }
        var d = copy(desired); d[key] = value; desired = d
        var q = copy(queued); q[key] = value; queued = q
        if (final) flush(); else if (!coalesce.running) coalesce.start()
    }
    function flush() {
        var q = queued; queued = ({})
        if (!available) return
        for (var key in q) {
            var id = send("set", {key:key,value:q[key]})
            var p = copy(pending); p[key] = id; pending = p
        }
    }
    function setDragging(key, active) { var d = copy(dragging); d[key] = active; dragging = d }
    function toggleSetting(key) { edit(key, value(key) ? 0 : 1, true) }
    function armWatchdog() {
        watchdog.interval = Math.max(3100, defaultsDeadline - Date.now() + 100)
        watchdog.restart()
    }
    function syncWatchdog(next) {
        if (next.present === false && next.stale === false && Object.keys(requests).length === 0) watchdog.stop()
        else armWatchdog()
    }
    function scheduleHighlights() {
        var soonest = Infinity
        for (var key in highlights) soonest = Math.min(soonest, highlights[key])
        if (soonest === Infinity) highlightExpiry.stop()
        else { highlightExpiry.interval = Math.max(1, soonest - Date.now()); highlightExpiry.restart() }
    }
    function acceptState(next) {
        lastSeen = Date.now(); stale = !!next.stale
        syncWatchdog(next)
        if ((next.connected && !next.stale) || next.present === false) transportError = ""
        var changed = false
        for (var key in next) if (next[key] !== state[key]) { changed = true; break }
        if (!changed) for (var oldKey in state) if (!(oldKey in next)) { changed = true; break }
        if (!changed) return // Heartbeat refreshes liveness, not every control's bindings.
        if (opened) {
            var h = copy(highlights), highlighted = false
            for (var field in next) {
                if (state[field] !== undefined && next[field] !== state[field] && !pending[field] && desired[field] === undefined && !dragging[field]) {
                    h[field] = Date.now() + 600; highlighted = true
                }
            }
            if (highlighted) { highlights = h; scheduleHighlights() }
        }
        if (next.present === false) {
            state = next; stale = false; error = ""
            desired = ({}); queued = ({}); pending = ({})
            phantomArmed = false; page = "controls"; helpOpen = false
        } else if (!next.connected || next.stale) {
            var retained = copy(state); for (var k in next) retained[k] = next[k]; state = retained
            stale = true; desired = ({}); queued = ({}); pending = ({})
        } else state = next
        if (next.error) transportError = next.error
    }
    function receive(line) {
        try {
            var msg = JSON.parse(line)
            if (msg.type === "state") {
                if (msg.seq !== undefined && msg.seq <= sequence) return
                if (msg.seq !== undefined) sequence = msg.seq
                acceptState(msg.state)
            } else if (msg.type === "result") {
                if (msg.id === defaultsId) { defaultsId = -1; defaultsDeadline = 0 }
                var r = copy(requests); delete r[msg.id]; requests = r
                if (msg.state) acceptState(msg.state); else syncWatchdog(state)
                var p = copy(pending), d = copy(desired)
                for (var key in p) if (p[key] === msg.id) {
                    delete p[key]
                    if (!dragging[key] && queued[key] === undefined) delete d[key]
                }
                pending = p; desired = d
                if (!msg.ok) error = msg.error || "The device rejected that change."
                else error = ""
            }
        } catch (e) { error = "Unreadable device response" }
    }
    function back() { phantomArmed = false; page = "controls"; Qt.callLater(function() { hardwareButton.forceActiveFocus() }) }
    Component.onCompleted: worker.running = true
    onOpenedChanged: {
        phantomArmed = false
        if (!opened) { dragging = ({}); flush(); highlights = ({}); highlightExpiry.stop() }
        send("poll", {intervalMs:opened ? 50 : 500})
    }
    Process {
        id: worker
        command: [Qt.resolvedUrl("libexec/wave-xlr-control").toString().replace("file://", ""), "--watch"]
        stdinEnabled: true
        stdout: SplitParser { onRead: function(line) { root.receive(line) } }
        onStarted: { root.lastSeen = Date.now(); root.sequence = -1; root.armWatchdog(); root.send("poll", {intervalMs:root.opened ? 50 : 500}) }
        onExited: {
            watchdog.stop()
            root.requests = ({}); root.defaultsDeadline = 0; root.defaultsId = -1
            root.stale = true; root.pending = ({}); root.desired = ({}); root.queued = ({})
            root.transportError = "Device worker stopped. Reconnecting…"; restart.start()
        }
    }
    Timer { id: restart; interval: 2000; onTriggered: worker.running = true }
    Timer { id: coalesce; interval: 50; onTriggered: root.flush() }
    Timer {
        id: watchdog
        interval: 3100
        onTriggered: {
            root.stale = true
            if (worker.running) {
                root.queued = ({}); root.pending = ({}); root.desired = ({}); root.requests = ({})
                root.transportError = "Device worker timed out. Reconnecting…"
                worker.signal(9)
            }
        }
    }
    Timer {
        id: highlightExpiry
        onTriggered: {
            var h = root.copy(root.highlights)
            for (var k in h) if (h[k] <= Date.now()) delete h[k]
            root.highlights = h
            root.scheduleHighlights()
        }
    }
    IpcHandler {
        target: "sudonim.wave-xlr-status"
        function status(): string { return JSON.stringify({state:root.state,stale:root.stale,workerRunning:worker.running,opened:root.opened,pending:root.pending,sequence:root.sequence,error:root.error || root.transportError,displayedGain:root.value("gain"),page:root.page,focus:root.focusedControl,pendingCount:Object.keys(root.pending).length,watchdogRunning:watchdog.running,requestCount:Object.keys(root.requests).length}) }
    }
    function ensureVisible(item) {
        if (item && item.objectName) focusedControl = item.objectName
        if (!item || !opened) return
        var y = item.mapToItem(content, 0, 0).y
        if (y < scroll.contentY) scroll.contentY = Math.max(0, y - Style.space(4))
        else if (y + item.height > scroll.contentY + scroll.height) scroll.contentY = Math.min(Math.max(0, content.height - scroll.height), y + item.height - scroll.height + Style.space(4))
    }
    component Label: Text {
        color: root.fg; font.family: root.family; font.pixelSize: Style.font.body
        textFormat: Text.PlainText
    }
    component Caption: Label { font.pixelSize: Style.font.caption; opacity: 0.7 }
    component Action: Button {
        focusable: true; foreground: root.fg; fontFamily: root.family
        objectName: text
        onActiveFocusChanged: if (activeFocus) root.ensureVisible(this)
        opacity: enabled ? 1 : 0.4
        implicitHeight: Math.max(Style.space(32), implicitContentHeight)
        property real implicitContentHeight: Style.space(32)
    }
    component Divider: Rectangle {
        width: parent.width; height: Math.max(1, Style.space(1)); color: root.fg; opacity: 0.16
    }
    component SliderField: Column {
        id: field
        property string controlKey
        property string label
        property real minimum: 0
        property real maximum: 100
        property real increment: 1
        property string unit: ""
        property bool hero: false
        property string leftCaption: ""
        property string rightCaption: ""
        readonly property real current: root.value(controlKey) !== undefined ? Number(root.value(controlKey)) : minimum
        width: parent.width; spacing: Style.space(3)
        Row {
            width: parent.width
            Label { width: parent.width - numeric.width; text: field.label; anchors.verticalCenter: parent.verticalCenter }
            Label {
                id: numeric
                text: root.value(field.controlKey) === undefined ? "—" : field.controlKey === "balance" ? (field.current === 100 ? "Centered" : field.current < 100 ? "Mic +" + Math.round(100-field.current) : "Computer +" + Math.round(field.current-100)) : String(Math.round(field.current * 100) / 100) + field.unit
                font.pixelSize: field.hero ? Style.font.displayLarge : Style.font.body
                color: root.highlights[field.controlKey] ? Color.accent : root.fg
            }
        }
        Row {
            width: parent.width; spacing: Style.space(6)
            Action { visible: field.hero; width: Style.space(28); text: "−"; enabled: root.supported(field.controlKey); onClicked: field.adjust(-1, false) }
            PanelSlider {
                id: slider
                objectName: field.controlKey
                onActiveFocusChanged: if (activeFocus) { root.focusedControl = field.controlKey; root.ensureVisible(field) }
                width: parent.width - (field.hero ? Style.space(68) : 0)
                bar: root.bar
                enabled: root.supported(field.controlKey)
                opacity: enabled ? 1 : 0.4
                activeFocusOnTab: true
                minimum: field.minimum; maximum: field.maximum; step: field.increment
                value: field.current
                fillColor: Color.accent; knobColor: root.fg
                trackColor: Util.alpha(root.fg, 0.18)
                tickCount: field.controlKey === "balance" ? 3 : 0
                onDraggingChanged: { root.setDragging(field.controlKey, dragging); if (dragging) forceActiveFocus() }
                onMoved: function(v) { root.edit(field.controlKey, field.snap(v), false) }
                onReleased: function(v) { root.edit(field.controlKey, field.snap(v), true) }
                Keys.onLeftPressed: function(e) { field.adjust(-1, true); e.accepted = true }
                Keys.onRightPressed: function(e) { field.adjust(1, true); e.accepted = true }
                Rectangle { anchors.fill: parent; color: "transparent"; border.color: Color.accent; visible: slider.activeFocus; radius: Style.cornerRadius }
                MouseArea {
                    anchors.fill: parent
                    acceptedButtons: Qt.NoButton
                    enabled: slider.enabled
                    onWheel: function(e) { field.adjust(e.angleDelta.y > 0 ? 1 : -1, !!(e.modifiers & Qt.ShiftModifier)); e.accepted = true }
                }
            }
            Action { visible: field.hero; width: Style.space(28); text: "+"; enabled: root.supported(field.controlKey); onClicked: field.adjust(1, false) }
        }
        Row {
            width: parent.width; visible: field.leftCaption !== ""
            Caption { width: parent.width / 2; text: field.leftCaption }
            Caption { width: parent.width / 2; horizontalAlignment: Text.AlignRight; text: field.rightCaption }
        }
        Connections {
            target: root
            function onOpenedChanged() { if (!root.opened) { slider.dragging = false; slider.liveValue = field.current } }
            function onAvailableChanged() { if (!root.available) { slider.dragging = false; slider.liveValue = field.current } }
        }
        function snap(v) { return Math.max(minimum, Math.min(maximum, Math.round(v/increment)*increment)) }
        function adjust(direction, fine) { root.edit(controlKey, snap(current + direction*(controlKey === "headphones" && !fine ? 1 : increment)), true) }
    }
    component Effect: Item {
        id: effect
        property string controlKey
        property string label
        width: parent.width; height: Style.space(34)
        Action {
            anchors.fill: parent; objectName: effect.controlKey; enabled: root.supported(effect.controlKey) && !root.pending[effect.controlKey]
            onClicked: root.toggleSetting(effect.controlKey)
            Accessible.name: effect.label + (root.value(effect.controlKey) ? " on" : " off")
        }
        Label { text: effect.label; anchors.left: parent.left; anchors.verticalCenter: parent.verticalCenter }
        Caption { anchors.right: switchView.left; anchors.rightMargin: Style.space(10); anchors.verticalCenter: parent.verticalCenter; text: root.pending[effect.controlKey] ? "…" : root.state[effect.controlKey] === undefined ? "—" : root.value(effect.controlKey) ? "On" : "Off" }
        ToggleSwitch {
            id: switchView; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter
            checked: !!root.value(effect.controlKey); interactive: false; trackHeight: Style.space(18)
            foreground: root.fg; accent: Color.accent
        }
    }
    BarIconButton {
        id: button
        anchors.fill: parent
        bar: root.bar
        text: ""
        iconComponent: Component {
            Canvas {
                id: deviceIcon
                readonly property color ink: root.state.mute ? Color.urgent : button.foreground
                readonly property bool muted: root.state.mute === true
                onInkChanged: requestPaint()
                onMutedChanged: requestPaint()
                onWidthChanged: requestPaint()
                onHeightChanged: requestPaint()
                onPaint: {
                    var ctx = getContext("2d")
                    ctx.reset()
                    ctx.scale(width / 24, height / 24)
                    ctx.strokeStyle = ink
                    ctx.fillStyle = ink
                    ctx.lineWidth = 1.65
                    ctx.lineCap = "round"
                    ctx.lineJoin = "round"
                    // Rounded interface chassis, central dial and three mode LEDs.
                    ctx.beginPath()
                    ctx.moveTo(5, 2.5); ctx.lineTo(19, 2.5)
                    ctx.quadraticCurveTo(21.5, 2.5, 21.5, 5)
                    ctx.lineTo(21.5, 19)
                    ctx.quadraticCurveTo(21.5, 21.5, 19, 21.5)
                    ctx.lineTo(5, 21.5)
                    ctx.quadraticCurveTo(2.5, 21.5, 2.5, 19)
                    ctx.lineTo(2.5, 5)
                    ctx.quadraticCurveTo(2.5, 2.5, 5, 2.5)
                    ctx.stroke()
                    ctx.beginPath(); ctx.arc(12, 10.5, 5, 0, Math.PI * 2); ctx.stroke()
                    ctx.beginPath(); ctx.moveTo(12, 6); ctx.lineTo(12, 8); ctx.stroke()
                    for (var i = 0; i < 3; ++i) {
                        ctx.beginPath(); ctx.arc(8 + i * 4, 18, 0.9, 0, Math.PI * 2); ctx.fill()
                    }
                    if (muted) {
                        ctx.beginPath(); ctx.moveTo(5, 19); ctx.lineTo(19, 5); ctx.stroke()
                    }
                }
            }
        }
        opacity: root.available ? 1 : 0.55
        tooltipText: root.available ? "Wave XLR MK.2 · " + root.state.gain + " dB" + (root.state.mute ? " · MUTED" : "") : "Wave XLR MK.2 · " + (root.transportError || root.error || (root.disconnected ? "Not connected" : "Reconnecting"))
        onPressed: root.toggle()
    }
    KeyboardPanel {
        id: popup
        anchorItem: button; owner: root; bar: root.bar; open: root.opened
        padding: Style.space(24)
        focusTarget: keys
        contentWidth: popup.fittedContentWidth(Style.space(392))
        contentHeight: popup.fittedContentHeight(content.implicitHeight, Style.space(820))
        FocusScope {
            id: keys; anchors.fill: parent
            Keys.onEscapePressed: { if (root.phantomArmed) { root.phantomArmed = false; phantomButton.forceActiveFocus() } else if (root.page !== "controls") root.back(); else root.close() }
            Keys.onPressed: function(e) {
                if (e.key === Qt.Key_M && !e.isAutoRepeat && !root.pending.mute && !root.phantomArmed && root.supported("mute")) { root.toggleSetting("mute"); e.accepted = true }
            }
            Flickable {
                id: scroll; anchors.fill: parent; contentHeight: content.implicitHeight; clip: true
                boundsBehavior: Flickable.StopAtBounds
                Controls.ScrollBar.vertical: Controls.ScrollBar {}
                Column {
                    id: content; width: parent.width; spacing: Style.space(14)
                    Row {
                        width: parent.width
                        Column {
                            width: parent.width - helpButton.width; spacing: Style.space(5)
                            Label { text: root.page === "controls" ? "Wave XLR MK.2" : "Hardware settings"; font.pixelSize: Style.font.heading; font.bold: true }
                            Caption { text: root.disconnected ? "Not connected" : root.stale ? "Reconnecting" : root.state.usb ? "Connected · USB" : "Connected · basic audio"; color: root.disconnected ? root.fg : root.stale ? Color.urgent : root.fg }
                        }
                        Action { id: helpButton; visible: !root.disconnected; text: "?"; tooltipText: "Controls and keyboard help"; bordered: true; onClicked: root.helpOpen = !root.helpOpen }
                    }
                    Caption {
                        width: parent.width; wrapMode: Text.WordWrap; visible: root.helpOpen
                        text: "M: mute mic. Tab: next control. ← →: adjust. Headphones: 0.25 dB per arrow key; wheel: 1 dB, Shift-wheel: 0.25 dB. Gain: 1 dB. Blend: 0 = mic, 100 = centered, 200 = computer. These are settings, not signal meters. Hardware changes highlight briefly."
                    }
                    Label {
                        width: parent.width; wrapMode: Text.WordWrap
                        visible: !root.disconnected && (root.error !== "" || root.transportError !== "" || root.stale)
                        text: root.error || root.transportError || "Last known settings · waiting for fresh device state"
                        color: Color.urgent; font.pixelSize: Style.font.caption
                    }
                    Caption {
                        width: parent.width; wrapMode: Text.WordWrap; visible: root.disconnected
                        text: "Connect your Wave XLR or dock your laptop. Controls will appear automatically."
                    }
                    Column {
                        visible: root.available && root.page === "controls"; width: parent.width; spacing: Style.space(14)
                        Row {
                            width: parent.width
                            Caption { width: parent.width - muteButton.width; text: "MICROPHONE"; anchors.verticalCenter: parent.verticalCenter; font.letterSpacing: 1 }
                            Action { id: muteButton; text: root.value("mute") ? "Unmute mic  M" : "Mute mic  M"; foreground: root.value("mute") ? Color.urgent : root.fg; bordered: true; enabled: root.supported("mute") && !root.pending.mute; onClicked: root.toggleSetting("mute") }
                        }
                        SliderField { controlKey: "gain"; label: "Gain"; minimum: 0; maximum: 80; increment: 1; unit: " dB"; hero: true; leftCaption: "0 dB"; rightCaption: "80 dB" }
                        Divider {}
                        Caption { text: "MONITORING"; font.letterSpacing: 1 }
                        SliderField { controlKey: "headphones"; label: "Headphones"; minimum: -60; maximum: 0; increment: 0.25; unit: " dB" }
                        SliderField { controlKey: "balance"; label: "Monitor blend"; minimum: 0; maximum: 200; increment: 1; leftCaption: "Mic"; rightCaption: "Computer" }
                        Divider {}
                        Caption { text: "ONBOARD PROCESSING"; font.letterSpacing: 1 }
                        Column {
                            width: parent.width; spacing: 0
                            Effect { controlKey: "clipguard"; label: "Clipguard" }
                            Effect { controlKey: "lowCut"; label: "Low cut" }
                            Effect { controlKey: "expander"; label: "Expander" }
                            Effect { controlKey: "compressor"; label: "Compressor" }
                            Effect { controlKey: "voiceTune"; label: "Voice Tune" }
                        }
                        SliderField { controlKey: "strength"; label: "Voice Tune strength"; increment: 1; unit: "%"; opacity: root.value("voiceTune") ? 1 : 0.55 }
                        Divider {}
                        Action {
                            id: hardwareButton; width: parent.width; leftAlign: true
                            text: "Hardware settings  ·  48V " + (root.state.phantom === undefined ? "—" : root.state.phantom ? "On" : "Off") + "  →"
                            onClicked: { root.page = "hardware"; scroll.contentY = 0; Qt.callLater(function() { backButton.forceActiveFocus() }) }
                        }
                    }
                    Column {
                        visible: root.available && root.page === "hardware"; width: parent.width; spacing: Style.space(18)
                        Row {
                            width: parent.width
                            Label { width: parent.width - phantomButton.width; text: "48V phantom power"; anchors.verticalCenter: parent.verticalCenter }
                            Action {
                                id: phantomButton; bordered: true
                                text: root.pending.phantom ? "Pending…" : (root.state.phantom === undefined ? "—" : root.state.phantom ? "On" : "Off") + " · Change"
                                enabled: root.supported("phantom") && !root.pending.phantom && !root.phantomArmed
                                onClicked: { root.phantomTarget = !root.state.phantom; root.phantomArmed = true; Qt.callLater(function() { cancelPhantom.forceActiveFocus() }) }
                            }
                        }
                        Caption { width: parent.width; wrapMode: Text.WordWrap; text: "Power for microphones that need it. Changing this may briefly mute the mic." }
                        Column {
                            visible: root.phantomArmed; width: parent.width; spacing: Style.space(10)
                            Label { width: parent.width; wrapMode: Text.WordWrap; text: "Turn 48V " + (root.phantomTarget ? "on" : "off") + "? Use phantom power only when your microphone requires it."; color: Color.urgent }
                            Row {
                                spacing: Style.space(8)
                                Action { id: cancelPhantom; text: "Cancel"; bordered: true; onClicked: { root.phantomArmed = false; phantomButton.forceActiveFocus() } }
                                Action { text: "Turn " + (root.phantomTarget ? "on" : "off"); bordered: true; foreground: Color.urgent; enabled: root.supported("phantom"); onClicked: { root.edit("phantom", root.phantomTarget ? 1 : 0, true); root.phantomArmed = false; phantomButton.forceActiveFocus() } }
                            }
                        }
                        Divider {}
                        Effect { controlKey: "lowImpedance"; label: "Low impedance mode" }
                        Caption { text: "Headphone output" }
                        Divider {}
                        Caption { text: "ABOUT THIS CONTROL PANEL"; font.letterSpacing: 1 }
                        Caption { width: parent.width; wrapMode: Text.WordWrap; text: "Controls the device’s onboard settings. EQ, lighting, Auto Gain and firmware updates aren’t available here. Wave Link app mixing and VST effects are separate." }
                        Action { text: "Use mic + headphones as defaults"; bordered: true; enabled: root.available; onClicked: root.send("defaults") }
                        Action { id: backButton; text: "← Back to controls"; foreground: Color.accent; onClicked: root.back() }
                    }
                    Row {
                        visible: !root.disconnected
                        width: parent.width
                        Caption { width: parent.width - refreshButton.width; anchors.verticalCenter: parent.verticalCenter; text: Object.keys(root.highlights).length ? "Device change" : Object.keys(root.pending).length ? "Saving…" : root.stale ? "Settings unavailable" : "Live hardware settings" }
                        Action { id: refreshButton; text: "Refresh"; enabled: worker.running; onClicked: { root.error = ""; root.send("refresh") } }
                    }
                }
            }
        }
    }
}
