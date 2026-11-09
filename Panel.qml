import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Shapes
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
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
    property int eqBand: 1 // The band the EQ page's editor and highlighted graph point show.
    readonly property color fg: Commons.Color.popups.text
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
    readonly property var eqTypes: ["High-pass", "Low shelf", "Bell", "Notch", "High shelf", "Low-pass"]
    function eqHasGain(type) { return type === 1 || type === 2 || type === 4 }
    // The device stores Q as round(64·log10 Q); report it as the worker does (3 significant digits).
    function qSnap(q) {
        var b = Math.max(-64, Math.min(64, Math.round(64 * Math.log10(q))))
        var v = Math.pow(10, b / 64), scale = Math.pow(10, 2 - Math.floor(Math.log10(v)))
        return Math.round(v * scale) / scale
    }
    function formatHz(f) { return f >= 1000 ? (Math.round(f / 100) / 10) + " kHz" : Math.round(f) + " Hz" }
    function eqUnavailableText() {
        var reason = state.eqUnavailable
        if (reason === "firmwareOld") return "Hardware EQ needs firmware 1.0.15 or later (this unit has " + (state.firmware || "older firmware") + "). Update it with Elgato Wave Link on Windows or macOS."
        if (reason === "unsupportedBlock") return "The device’s EQ settings weren’t recognized, so the EQ is left untouched."
        return "Firmware version couldn’t be read; hardware EQ is unavailable."
    }
    // The EQ settings as one string, so the curves recompute only when a band changes,
    // not on every state update. Empty when the device has no usable EQ.
    function eqSignature() {
        if (state.eq === undefined) return ""
        var s = value("eq") ? "1" : "0"
        for (var n = 1; n <= 4; n++) {
            var p = "eq" + n
            s += "|" + [value(p + "On") ? 1 : 0, value(p + "Type"), value(p + "Freq"), value(p + "Gain"), value(p + "Q")].join(",")
        }
        return s
    }
    function eqBands(signature) {
        return signature === "" ? [] : signature.split("|").slice(1).map(function(part) {
            var f = part.split(",").map(Number)
            return {on:f[0] === 1, type:f[1], freq:f[2], gain:f[3], q:f[4]}
        })
    }
    // RBJ audio-EQ-cookbook biquads at the device's 48 kHz rate: [b0, b1, b2, a0, a1, a2].
    function eqBiquad(band) {
        var A = Math.pow(10, band.gain / 40), w = 2 * Math.PI * band.freq / 48000
        var c = Math.cos(w), s = Math.sin(w), al = s / (2 * band.q), sq = 2 * Math.sqrt(A) * al
        switch (band.type) {
        case 0: return [(1 + c) / 2, -(1 + c), (1 + c) / 2, 1 + al, -2 * c, 1 - al]
        case 1: return [A * ((A + 1) - (A - 1) * c + sq), 2 * A * ((A - 1) - (A + 1) * c), A * ((A + 1) - (A - 1) * c - sq),
                        (A + 1) + (A - 1) * c + sq, -2 * ((A - 1) + (A + 1) * c), (A + 1) + (A - 1) * c - sq]
        case 3: return [1, -2 * c, 1, 1 + al, -2 * c, 1 - al]
        case 4: return [A * ((A + 1) + (A - 1) * c + sq), -2 * A * ((A - 1) + (A + 1) * c), A * ((A + 1) + (A - 1) * c - sq),
                        (A + 1) - (A - 1) * c + sq, 2 * ((A - 1) - (A + 1) * c), (A + 1) - (A - 1) * c - sq]
        case 5: return [(1 - c) / 2, 1 - c, (1 - c) / 2, 1 + al, -2 * c, 1 - al]
        default: return [1 + al * A, -2 * c, 1 - al * A, 1 + al / A, -2 * c, 1 - al / A]
        }
    }
    function eqDb(freq, filters) {
        var w = 2 * Math.PI * freq / 48000, c1 = Math.cos(w), s1 = Math.sin(w), c2 = Math.cos(2 * w), s2 = Math.sin(2 * w), db = 0
        for (var i = 0; i < filters.length; i++) {
            var k = filters[i]
            var nr = k[0] + k[1] * c1 + k[2] * c2, ni = k[1] * s1 + k[2] * s2
            var dr = k[3] + k[4] * c1 + k[5] * c2, di = k[4] * s1 + k[5] * s2
            db += 10 * Math.log10((nr * nr + ni * ni) / (dr * dr + di * di))
        }
        return db
    }
    function eqX(freq, width) { return Math.log10(freq / 20) / 3 * width }
    function eqY(db, height, range) { return height / 2 - Math.max(-range, Math.min(range, db)) / range * height / 2 }
    // The summed response of the enabled bands as polyline points; `closed` adds the 0 dB baseline for a fill.
    function eqCurve(signature, width, height, range, closed) {
        var filters = eqBands(signature).filter(function(b) { return b.on }).map(eqBiquad), points = []
        if (signature === "" || width <= 0) return points
        for (var i = 0; i <= 96; i++) {
            var f = 20 * Math.pow(1000, i / 96)
            points.push(Qt.point(eqX(f, width), eqY(eqDb(f, filters), height, range)))
        }
        if (closed) points.push(Qt.point(width, height / 2), Qt.point(0, height / 2), points[0])
        return points
    }
    function openPage(name) {
        page = name; scroll.contentY = 0
        Qt.callLater(function() { backButton.forceActiveFocus() })
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
                if (msg.state) acceptState(msg.state)
                else if (msg.patch) { // Only the changed fields of a steady connected state.
                    var merged = copy(state)
                    for (var field in msg.patch) merged[field] = msg.patch[field]
                    acceptState(merged)
                } else { lastSeen = Date.now(); syncWatchdog(state) } // Liveness-only heartbeat: nothing changed.
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
    function back() {
        var from = page
        phantomArmed = false
        if (from === "eq") { dragging = ({}); flush() } // The EQ page's sliders are destroyed on leaving.
        page = "controls"
        Qt.callLater(function() { (from === "eq" ? eqButton : hardwareButton).focusAction() })
    }
    Component.onCompleted: worker.running = true
    onOpenedChanged: {
        phantomArmed = false
        if (opened) scroll.contentY = 0
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
        property bool logarithmic: false
        property real logStep: 0.01
        property string quantize: ""
        property string display: ""
        property bool editable: true
        readonly property bool live: root.supported(controlKey) && editable
        readonly property real current: root.value(controlKey) !== undefined ? Number(root.value(controlKey)) : minimum
        readonly property string valueText: root.value(controlKey) === undefined ? "—" : display !== "" ? display : controlKey === "balance" ? (current === 100 ? "Centered" : current < 100 ? "Mic +" + Math.round(100-current) : "Computer +" + Math.round(current-100)) : String(Math.round(current * 100) / 100) + (hero ? "" : unit)
        width: parent.width; spacing: Style.space(hero ? 6 : 3)
        Item {
            width: parent.width; height: Math.max(name.implicitHeight, numeric.implicitHeight)
            Label { id: name; anchors.left: parent.left; anchors.baseline: numeric.baseline; text: field.label; opacity: field.hero ? 0.7 : 1 }
            Label {
                id: numeric
                anchors.right: unitLabel.visible ? unitLabel.left : parent.right; anchors.rightMargin: unitLabel.visible ? Style.space(3) : 0
                anchors.verticalCenter: parent.verticalCenter
                text: field.valueText
                font.pixelSize: field.hero ? Style.font.displayLarge : Style.font.body
                font.weight: field.hero ? Font.Light : Font.Normal
                opacity: field.hero ? 1 : 0.8
                color: root.highlights[field.controlKey] ? Commons.Color.accent : root.fg
            }
            Label { id: unitLabel; visible: field.hero && field.valueText !== "—"; anchors.right: parent.right; anchors.baseline: numeric.baseline; text: field.unit.trim(); opacity: 0.7 }
        }
        PanelSlider {
            id: slider
            objectName: field.controlKey
            onActiveFocusChanged: if (activeFocus) { root.focusedControl = field.controlKey; root.ensureVisible(field) }
            width: parent.width
            bar: root.bar
            enabled: field.live
            opacity: enabled ? 1 : 0.4
            activeFocusOnTab: true
            minimum: field.toSlider(field.minimum); maximum: field.toSlider(field.maximum)
            step: field.logarithmic ? field.logStep : field.increment
            value: field.toSlider(field.current)
            fillColor: Commons.Color.accent; knobColor: root.fg
            trackColor: Util.alpha(root.fg, 0.18)
            tickCount: field.controlKey === "balance" ? 3 : 0
            onDraggingChanged: { root.setDragging(field.controlKey, dragging); if (dragging) forceActiveFocus() }
            onMoved: function(v) { root.edit(field.controlKey, field.snap(field.fromSlider(v)), false) }
            onReleased: function(v) { root.edit(field.controlKey, field.snap(field.fromSlider(v)), true) }
            Keys.onLeftPressed: function(e) { field.adjust(-1, true); e.accepted = true }
            Keys.onRightPressed: function(e) { field.adjust(1, true); e.accepted = true }
            Rectangle { anchors.fill: parent; color: "transparent"; border.color: Commons.Color.accent; visible: slider.activeFocus; radius: Style.cornerRadius }
            MouseArea {
                anchors.fill: parent
                acceptedButtons: Qt.NoButton
                enabled: slider.enabled
                onWheel: function(e) { field.adjust(e.angleDelta.y > 0 ? 1 : -1, !!(e.modifiers & Qt.ShiftModifier)); e.accepted = true }
            }
        }
        Row {
            width: parent.width; visible: field.leftCaption !== ""
            Caption { width: parent.width / 2; text: field.leftCaption }
            Caption { width: parent.width / 2; horizontalAlignment: Text.AlignRight; text: field.rightCaption }
        }
        Connections {
            target: root
            function onOpenedChanged() { if (!root.opened) { slider.dragging = false; slider.liveValue = field.toSlider(field.current) } }
            function onAvailableChanged() { if (!root.available) { slider.dragging = false; slider.liveValue = field.toSlider(field.current) } }
        }
        function toSlider(v) { return logarithmic ? Math.log10(v) : v }
        function fromSlider(v) { return logarithmic ? Math.pow(10, v) : v }
        function snap(v) {
            v = Math.max(minimum, Math.min(maximum, v))
            return quantize === "q" ? root.qSnap(v) : Math.max(minimum, Math.min(maximum, Math.round(v/increment)*increment))
        }
        function adjust(direction, fine) {
            if (!live) return
            if (logarithmic) {
                var next = snap(Math.pow(10, Math.log10(current) + direction * logStep))
                if (next === current) next = snap(current + direction * increment)
                root.edit(controlKey, next, true)
                return
            }
            root.edit(controlKey, snap(current + direction*(controlKey === "headphones" && !fine ? 1 : increment)), true)
        }
    }
    // A full-width setting row: label, optional explanation, and a switch.
    component Effect: Item {
        id: effect
        property string controlKey
        property string label
        property string detail: ""
        width: parent.width; height: Math.max(Style.space(34), effectText.implicitHeight + Style.space(16))
        Action {
            anchors.fill: parent; objectName: effect.controlKey; enabled: root.supported(effect.controlKey) && !root.pending[effect.controlKey]
            onClicked: root.toggleSetting(effect.controlKey)
            Accessible.name: effect.label + (root.value(effect.controlKey) ? " on" : " off")
        }
        Column {
            id: effectText; spacing: Style.space(2)
            anchors.left: parent.left; anchors.right: effectSwitch.left; anchors.rightMargin: Style.space(12); anchors.verticalCenter: parent.verticalCenter
            Label { width: parent.width; text: effect.label; elide: Text.ElideRight }
            Caption { width: parent.width; visible: effect.detail !== ""; text: effect.detail; wrapMode: Text.WordWrap }
        }
        ToggleSwitch {
            id: effectSwitch; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter
            checked: !!root.value(effect.controlKey); busy: !!root.pending[effect.controlKey]; interactive: false; cursorRing: false
            trackHeight: Style.space(16); foreground: root.fg; accent: Commons.Color.accent
            opacity: root.state[effect.controlKey] === undefined ? 0.4 : 1
        }
    }
    // A half-width processing toggle: filled while on, outlined while off.
    component Chip: Rectangle {
        id: chip
        property string controlKey
        property string label
        readonly property bool on: !!root.value(controlKey)
        width: (parent.width - Style.space(8)) / 2; height: Style.space(36)
        radius: Style.cornerRadius
        color: on ? Util.alpha(root.fg, 0.08) : "transparent"
        border.width: on ? 0 : Math.max(1, Style.space(1)); border.color: Util.alpha(root.fg, 0.16)
        Action {
            anchors.fill: parent; objectName: chip.controlKey; enabled: root.supported(chip.controlKey) && !root.pending[chip.controlKey]
            onClicked: root.toggleSetting(chip.controlKey)
            Accessible.name: chip.label + (chip.on ? " on" : " off")
        }
        Label {
            anchors.left: parent.left; anchors.leftMargin: Style.space(10); anchors.right: chipSwitch.left; anchors.rightMargin: Style.space(6)
            anchors.verticalCenter: parent.verticalCenter; text: chip.label; elide: Text.ElideRight; opacity: chip.on ? 1 : 0.7
        }
        ToggleSwitch {
            id: chipSwitch; anchors.right: parent.right; anchors.rightMargin: Style.space(8); anchors.verticalCenter: parent.verticalCenter
            checked: chip.on; busy: !!root.pending[chip.controlKey]; interactive: false; cursorRing: false
            trackHeight: Style.space(14); foreground: root.fg; accent: Commons.Color.accent
        }
    }
    // A link to a sub-page: title, one line of what is inside, an optional preview, a chevron.
    component NavRow: Item {
        id: nav
        property string title
        property string detail
        property bool active: true
        default property alias trailing: trailingSlot.data
        signal activated()
        function focusAction() { navAction.forceActiveFocus() }
        width: parent.width; height: Math.max(Style.space(48), navText.implicitHeight + Style.space(20))
        Action {
            id: navAction; anchors.fill: parent; objectName: nav.title; enabled: nav.active
            onClicked: nav.activated()
            Accessible.name: nav.title + ". " + nav.detail
        }
        Column {
            id: navText; spacing: Style.space(2)
            anchors.left: parent.left; anchors.leftMargin: Style.space(2); anchors.right: trailingSlot.left; anchors.rightMargin: Style.space(10); anchors.verticalCenter: parent.verticalCenter
            Label { width: parent.width; text: nav.title; font.weight: Font.Medium; elide: Text.ElideRight; opacity: nav.active ? 1 : 0.6 }
            Caption { width: parent.width; text: nav.detail; wrapMode: Text.WordWrap }
        }
        Item {
            id: trailingSlot; anchors.right: chevron.left; anchors.rightMargin: Style.space(10); anchors.verticalCenter: parent.verticalCenter
            width: childrenRect.width; height: childrenRect.height
        }
        Label { id: chevron; anchors.right: parent.right; anchors.rightMargin: Style.space(4); anchors.verticalCenter: parent.verticalCenter; text: "›"; font.pixelSize: Style.font.heading; opacity: nav.active ? 0.6 : 0 }
    }
    // The summed EQ response. `editable` adds the grid, axis labels and draggable band points.
    component EqGraph: Item {
        id: graph
        property bool editable: false
        property real range: 12
        readonly property string signature: root.eqSignature()
        readonly property bool enabledEq: signature.charAt(0) === "1"
        readonly property var bands: root.eqBands(signature)
        property int dragBand: 0
        Repeater {
            model: graph.editable ? [100, 1000, 10000] : []
            Rectangle { x: root.eqX(modelData, graph.width); width: 1; height: graph.height; color: root.fg; opacity: 0.12 }
        }
        Repeater {
            model: graph.editable ? [-6, 0, 6] : [0]
            Rectangle { y: root.eqY(modelData, graph.height, graph.range); width: graph.width; height: 1; color: root.fg; opacity: modelData === 0 ? 0.2 : 0.08 }
        }
        Shape {
            anchors.fill: parent
            preferredRendererType: Shape.CurveRenderer
            opacity: graph.enabledEq ? 1 : 0.5
            ShapePath {
                strokeColor: "transparent"
                fillColor: graph.editable && graph.enabledEq ? Util.alpha(Commons.Color.accent, 0.12) : "transparent"
                PathPolyline { path: graph.editable ? root.eqCurve(graph.signature, graph.width, graph.height, graph.range, true) : [] }
            }
            ShapePath {
                strokeColor: graph.enabledEq ? Commons.Color.accent : root.fg
                strokeWidth: graph.editable ? 2 : 1.5
                strokeStyle: graph.enabledEq ? ShapePath.SolidLine : ShapePath.DashLine
                dashPattern: [3, 2]
                fillColor: "transparent"; joinStyle: ShapePath.RoundJoin; capStyle: ShapePath.RoundCap
                PathPolyline { path: root.eqCurve(graph.signature, graph.width, graph.height, graph.range, false) }
            }
        }
        Repeater {
            model: graph.editable ? graph.bands : []
            Rectangle {
                id: point
                readonly property int number: index + 1
                readonly property bool selected: number === root.eqBand
                readonly property real size: Style.space(selected ? 18 : 16)
                x: root.eqX(modelData.freq, graph.width) - size / 2
                y: root.eqY(root.eqHasGain(modelData.type) ? modelData.gain : 0, graph.height, graph.range) - size / 2
                width: size; height: size; radius: size / 2
                color: selected ? Commons.Color.accent : Commons.Color.popups.background
                border.width: selected ? 0 : Math.max(1, Style.space(1.5)); border.color: root.fg
                opacity: modelData.on ? 1 : 0.45
                Label {
                    anchors.centerIn: parent; text: point.number; font.pixelSize: Style.font.caption; font.bold: true
                    color: point.selected ? Commons.Color.popups.background : root.fg
                }
            }
        }
        MouseArea {
            id: dragArea
            anchors.fill: parent
            enabled: graph.editable && root.supported("eq" + root.eqBand + "Freq")
            preventStealing: true
            function nearest(mx, my) {
                var best = 0, bestDistance = Style.space(24)
                for (var i = 0; i < graph.bands.length; i++) {
                    var b = graph.bands[i]
                    var dx = root.eqX(b.freq, graph.width) - mx, dy = root.eqY(root.eqHasGain(b.type) ? b.gain : 0, graph.height, graph.range) - my
                    var distance = Math.sqrt(dx * dx + dy * dy)
                    if (distance < bestDistance) { best = i + 1; bestDistance = distance }
                }
                return best
            }
            function apply(mx, my, final) {
                var n = graph.dragBand, p = "eq" + n
                var freq = Math.round(Math.max(20, Math.min(20000, 20 * Math.pow(1000, mx / graph.width))))
                root.edit(p + "Freq", freq, final)
                if (root.eqHasGain(graph.bands[n - 1].type)) {
                    var gain = (graph.height / 2 - my) / (graph.height / 2) * graph.range
                    root.edit(p + "Gain", Math.max(-12, Math.min(12, Math.round(gain * 4) / 4)), final)
                }
            }
            function release() {
                var p = "eq" + graph.dragBand
                graph.dragBand = 0
                root.setDragging(p + "Freq", false); root.setDragging(p + "Gain", false)
            }
            onPressed: function(e) {
                var n = nearest(e.x, e.y)
                if (!n) { e.accepted = false; return }
                root.eqBand = n; graph.dragBand = n
                root.setDragging("eq" + n + "Freq", true); root.setDragging("eq" + n + "Gain", true)
            }
            onPositionChanged: function(e) { if (graph.dragBand) apply(e.x, e.y, false) }
            onReleased: function(e) { if (graph.dragBand) { apply(e.x, e.y, true); release() } }
            onCanceled: if (graph.dragBand) { root.flush(); release() }
            onWheel: function(e) {
                var p = "eq" + root.eqBand, q = Number(root.value(p + "Q"))
                var next = root.qSnap(Math.max(0.1, Math.min(10, Math.pow(10, Math.log10(q) + (e.angleDelta.y > 0 ? 1 : -1) / 64))))
                root.edit(p + "Q", next, true); e.accepted = true
            }
        }
        Connections {
            target: root
            function onOpenedChanged() { if (!root.opened && graph.dragBand) dragArea.release() }
        }
    }
    component BandTab: Rectangle {
        id: tab
        property int number
        readonly property bool selected: root.eqBand === number
        readonly property string prefix: "eq" + number
        width: (parent.width - 3 * Style.space(4)) / 4; height: Style.space(42)
        radius: Style.cornerRadius
        color: selected ? Util.alpha(root.fg, 0.1) : "transparent"
        Action {
            anchors.fill: parent; objectName: "eqBand" + tab.number
            onClicked: root.eqBand = tab.number
            Accessible.name: "Band " + tab.number + ", " + (root.eqTypes[root.value(tab.prefix + "Type")] || "") + (root.value(tab.prefix + "On") ? "" : ", off")
        }
        Column {
            anchors.centerIn: parent; spacing: Style.space(1)
            opacity: root.value(tab.prefix + "On") ? 1 : 0.5
            Label { anchors.horizontalCenter: parent.horizontalCenter; text: tab.number; font.bold: true; color: tab.selected ? Commons.Color.accent : root.fg }
            Caption { anchors.horizontalCenter: parent.horizontalCenter; text: root.eqTypes[root.value(tab.prefix + "Type")] || "—"; opacity: tab.selected ? 1 : 0.7 }
        }
    }
    component EqBand: Column {
        id: band
        property int number
        readonly property string prefix: "eq" + number
        readonly property int type: root.value(prefix + "Type") !== undefined ? Number(root.value(prefix + "Type")) : 2
        readonly property bool active: !!root.value(prefix + "On")
        width: parent.width; spacing: Style.space(10)
        Item {
            width: parent.width; height: Style.space(34)
            Row {
                anchors.left: parent.left; anchors.verticalCenter: parent.verticalCenter; spacing: Style.space(4)
                Action {
                    id: typePrev; width: Style.space(28); text: "‹"; objectName: band.prefix + "TypePrev"
                    Accessible.name: "Previous filter type, band " + band.number
                    enabled: root.supported(band.prefix + "Type") && !root.pending[band.prefix + "Type"]
                    onClicked: root.edit(band.prefix + "Type", (band.type + 5) % 6, true)
                }
                Label {
                    width: Style.space(84); horizontalAlignment: Text.AlignHCenter; anchors.verticalCenter: parent.verticalCenter
                    text: root.value(band.prefix + "Type") === undefined ? "—" : root.eqTypes[band.type] || "—"
                    color: root.highlights[band.prefix + "Type"] ? Commons.Color.accent : root.fg
                }
                Action {
                    id: typeNext; width: Style.space(28); text: "›"; objectName: band.prefix + "TypeNext"
                    Accessible.name: "Next filter type, band " + band.number
                    enabled: root.supported(band.prefix + "Type") && !root.pending[band.prefix + "Type"]
                    onClicked: root.edit(band.prefix + "Type", (band.type + 1) % 6, true)
                }
            }
            Effect { anchors.right: parent.right; width: Style.space(120); controlKey: band.prefix + "On"; label: "Band " + band.number }
        }
        Column {
            width: parent.width; spacing: Style.space(12)
            opacity: band.active ? 1 : 0.55
            SliderField {
                controlKey: band.prefix + "Freq"; label: "Frequency"; minimum: 20; maximum: 20000; increment: 1
                logarithmic: true; logStep: Math.log10(2) / 12; display: root.formatHz(current)
            }
            SliderField {
                visible: root.eqHasGain(band.type)
                controlKey: band.prefix + "Gain"; label: "Gain"; minimum: -12; maximum: 12; increment: 0.25; unit: " dB"
            }
            Caption {
                visible: !root.eqHasGain(band.type); width: parent.width; wrapMode: Text.WordWrap
                text: (root.eqTypes[band.type] || "This filter") + " has no gain; drag its point to move the frequency."
            }
            SliderField {
                controlKey: band.prefix + "Q"; label: "Width (Q)"; minimum: 0.1; maximum: 10; increment: 0.01
                logarithmic: true; logStep: 1 / 64; quantize: "q"; display: current.toFixed(2)
            }
        }
    }
    BarIconButton {
        id: button
        anchors.fill: parent
        bar: root.bar
        text: ""
        iconComponent: Component {
            DeviceIcon {
                ink: root.state.mute ? Commons.Color.urgent : button.foreground
                muted: root.state.mute === true
            }
        }
        opacity: root.available ? 1 : 0.55
        tooltipText: root.available ? "Elgato Wave XLR MK.2 · " + root.state.gain + " dB" + (root.state.mute ? " · MUTED" : "") : "Elgato Wave XLR MK.2 · " + (root.transportError || root.error || (root.disconnected ? "Not connected" : "Reconnecting"))
        onPressed: root.toggle()
    }
    KeyboardPanel {
        id: popup
        anchorItem: button; owner: root; bar: root.bar; open: root.opened
        padding: Style.space(20)
        focusTarget: keys
        contentWidth: popup.fittedContentWidth(Style.space(352))
        contentHeight: popup.fittedContentHeight(header.height + Style.space(16) + content.implicitHeight, Style.space(820))
        FocusScope {
            id: keys; anchors.fill: parent
            Keys.onEscapePressed: { if (root.phantomArmed) { root.phantomArmed = false; phantomButton.forceActiveFocus() } else if (root.page !== "controls") root.back(); else root.close() }
            Keys.onPressed: function(e) {
                if (e.key === Qt.Key_M && !e.isAutoRepeat && !root.pending.mute && !root.phantomArmed && root.supported("mute")) { root.toggleSetting("mute"); e.accepted = true }
            }
            // The header sits outside the scroll area so it can never scroll out of view.
            Item {
                id: header; width: parent.width; height: Math.max(Style.space(36), headerText.implicitHeight)
                Action {
                    id: backButton; visible: root.page !== "controls"; width: Style.space(30); height: Style.space(30); implicitContentHeight: Style.space(30)
                    anchors.left: parent.left; anchors.verticalCenter: parent.verticalCenter
                    text: "‹"; bordered: true; objectName: "back"; Accessible.name: "Back to controls"; onClicked: root.back()
                }
                DeviceIcon { id: headingIcon; visible: root.page === "controls"; width: Style.space(26); height: width; ink: root.fg; anchors.left: parent.left; anchors.verticalCenter: parent.verticalCenter }
                Column {
                    id: headerText; spacing: Style.space(2)
                    anchors.left: root.page === "controls" ? headingIcon.right : backButton.right; anchors.leftMargin: Style.space(10)
                    anchors.right: headerActions.left; anchors.rightMargin: Style.space(8); anchors.verticalCenter: parent.verticalCenter
                    Label { width: parent.width; elide: Text.ElideRight; text: root.page === "eq" ? "Equalizer" : root.page === "hardware" ? "Hardware" : "Wave XLR MK.2"; font.pixelSize: Style.font.title; font.bold: true }
                    Caption {
                        width: parent.width; elide: Text.ElideRight
                        text: root.disconnected ? "Not connected" : root.stale ? "Reconnecting" : Object.keys(root.pending).length ? "Saving…" : Object.keys(root.highlights).length ? "Changed on the device"
                            : root.page === "eq" ? "Runs on the device · every app hears it" : root.page === "hardware" ? "Firmware " + (root.state.firmware || "unknown")
                            : root.state.usb ? "Connected · USB" : "Connected · basic audio"
                        color: root.stale && !root.disconnected ? Commons.Color.urgent : root.fg
                    }
                }
                Row {
                    id: headerActions; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter; spacing: Style.space(6)
                    Action {
                        id: muteButton; visible: root.page === "controls" && root.available; bordered: true
                        text: root.value("mute") ? "Unmute  M" : "Mute  M"; Accessible.name: root.value("mute") ? "Unmute microphone" : "Mute microphone"
                        foreground: root.value("mute") ? Commons.Color.urgent : root.fg
                        enabled: root.supported("mute") && !root.pending.mute; onClicked: root.toggleSetting("mute")
                    }
                    Item {
                        visible: root.page === "eq" && root.available; width: eqSwitch.width + Style.space(8); height: Style.space(30)
                        Action {
                            anchors.fill: parent; objectName: "eq"; Accessible.name: "Equalizer " + (root.value("eq") ? "on" : "off")
                            enabled: root.supported("eq") && !root.pending.eq; onClicked: root.toggleSetting("eq")
                        }
                        ToggleSwitch {
                            id: eqSwitch; anchors.centerIn: parent; checked: !!root.value("eq"); busy: !!root.pending.eq; interactive: false; cursorRing: false
                            trackHeight: Style.space(18); foreground: root.fg; accent: Commons.Color.accent
                        }
                    }
                    Action { id: helpButton; visible: !root.disconnected; width: Style.space(30); text: "?"; tooltipText: "Controls and keyboard help"; Accessible.name: "Help"; onClicked: root.helpOpen = !root.helpOpen }
                }
            }
            Flickable {
                id: scroll; anchors.top: header.bottom; anchors.topMargin: Style.space(16); anchors.left: parent.left; anchors.right: parent.right; anchors.bottom: parent.bottom
                contentHeight: content.implicitHeight; clip: true
                boundsBehavior: Flickable.StopAtBounds
                Controls.ScrollBar.vertical: Controls.ScrollBar {}
                Column {
                    id: content; width: parent.width; spacing: Style.space(16)
                    Caption {
                        width: parent.width; wrapMode: Text.WordWrap; visible: root.helpOpen
                        text: "M: mute mic. Tab: next control. ← →: adjust. Esc: back. Headphones: 0.25 dB per arrow key; wheel: 1 dB, Shift-wheel: 0.25 dB. Gain: 1 dB. EQ graph: drag a point for frequency and gain, scroll for width. EQ frequency steps 1/12 octave; Q steps match the device. These are settings, not signal meters."
                    }
                    Label {
                        width: parent.width; wrapMode: Text.WordWrap
                        visible: !root.disconnected && (root.error !== "" || root.transportError !== "" || root.stale)
                        text: root.error || root.transportError || "Last known settings · waiting for fresh device state"
                        color: Commons.Color.urgent; font.pixelSize: Style.font.caption
                    }
                    Caption {
                        width: parent.width; wrapMode: Text.WordWrap; visible: root.disconnected
                        text: "Connect your Elgato Wave XLR MK.2 or dock your laptop. Controls will appear automatically."
                    }
                    Column {
                        visible: root.available && root.page === "controls"; width: parent.width; spacing: Style.space(16)
                        SliderField { controlKey: "gain"; label: "Mic gain"; minimum: 0; maximum: 80; increment: 1; unit: " dB"; hero: true; leftCaption: "0"; rightCaption: "80 dB" }
                        Divider {}
                        SliderField { controlKey: "headphones"; label: "Headphones"; minimum: -60; maximum: 0; increment: 0.25; unit: " dB" }
                        SliderField { controlKey: "balance"; label: "Monitor mix"; minimum: 0; maximum: 200; increment: 1; leftCaption: "Mic"; rightCaption: "Computer" }
                        Divider {}
                        Column {
                            width: parent.width; spacing: Style.space(8)
                            Caption { text: "ONBOARD PROCESSING"; font.letterSpacing: 1 }
                            Grid {
                                width: parent.width; columns: 2; spacing: Style.space(8)
                                Chip { controlKey: "clipguard"; label: "Clipguard" }
                                Chip { controlKey: "lowCut"; label: "Low cut" }
                                Chip { controlKey: "expander"; label: "Expander" }
                                Chip { controlKey: "compressor"; label: "Compressor" }
                            }
                            Rectangle {
                                width: parent.width; height: voiceTune.implicitHeight + Style.space(16)
                                radius: Style.cornerRadius
                                color: root.value("voiceTune") ? Util.alpha(root.fg, 0.08) : "transparent"
                                border.width: root.value("voiceTune") ? 0 : Math.max(1, Style.space(1)); border.color: Util.alpha(root.fg, 0.16)
                                Column {
                                    id: voiceTune; x: Style.space(10); y: Style.space(4); width: parent.width - Style.space(18); spacing: Style.space(2)
                                    Effect { controlKey: "voiceTune"; label: "Voice Tune" }
                                    SliderField { controlKey: "strength"; label: "Strength"; increment: 1; unit: "%"; opacity: root.value("voiceTune") ? 1 : 0.55 }
                                }
                            }
                        }
                        Column {
                            width: parent.width
                            Divider {}
                            NavRow {
                                id: eqButton; visible: root.state.usb === true
                                title: "Equalizer"; active: root.state.eq !== undefined
                                detail: !active ? root.eqUnavailableText()
                                    : root.value("eq") ? "On · " + [1, 2, 3, 4].filter(function(n) { return root.value("eq" + n + "On") }).length + " of 4 bands" : "Off · bands kept"
                                onActivated: root.openPage("eq")
                                EqGraph { visible: eqButton.active; width: Style.space(84); height: Style.space(26) }
                            }
                            Divider { visible: eqButton.visible }
                            NavRow {
                                id: hardwareButton; title: "Hardware"; detail: "Phantom power, headphones, firmware"
                                onActivated: root.openPage("hardware")
                                Label { visible: root.state.phantom === true; text: "48V"; color: Commons.Color.accent; font.pixelSize: Style.font.caption; font.bold: true }
                            }
                        }
                    }
                    Column {
                        visible: root.available && root.page === "hardware"; width: parent.width; spacing: Style.space(4)
                        Item {
                            width: parent.width; height: Math.max(phantomButton.height, phantomText.implicitHeight) + Style.space(16)
                            Column {
                                id: phantomText; spacing: Style.space(2)
                                anchors.left: parent.left; anchors.right: phantomButton.left; anchors.rightMargin: Style.space(12); anchors.verticalCenter: parent.verticalCenter
                                Label { width: parent.width; text: "48V phantom power" }
                                Caption { width: parent.width; wrapMode: Text.WordWrap; text: "For microphones that need it. Switching may briefly mute the mic." }
                            }
                            Action {
                                id: phantomButton; bordered: true; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter
                                text: root.pending.phantom ? "Pending…" : (root.state.phantom === undefined ? "—" : root.state.phantom ? "On" : "Off") + " · Change"
                                foreground: root.state.phantom ? Commons.Color.accent : root.fg
                                enabled: root.supported("phantom") && !root.pending.phantom && !root.phantomArmed
                                onClicked: { root.phantomTarget = !root.state.phantom; root.phantomArmed = true; Qt.callLater(function() { cancelPhantom.forceActiveFocus() }) }
                            }
                        }
                        Column {
                            visible: root.phantomArmed; width: parent.width; spacing: Style.space(10)
                            Label { width: parent.width; wrapMode: Text.WordWrap; text: "Turn 48V " + (root.phantomTarget ? "on" : "off") + "? Use phantom power only when your microphone requires it."; color: Commons.Color.urgent }
                            Row {
                                spacing: Style.space(8)
                                Action { id: cancelPhantom; text: "Cancel"; bordered: true; onClicked: { root.phantomArmed = false; phantomButton.forceActiveFocus() } }
                                Action { text: "Turn " + (root.phantomTarget ? "on" : "off"); bordered: true; foreground: Commons.Color.urgent; enabled: root.supported("phantom"); onClicked: { root.edit("phantom", root.phantomTarget ? 1 : 0, true); root.phantomArmed = false; phantomButton.forceActiveFocus() } }
                            }
                        }
                        Divider {}
                        Effect { controlKey: "lowImpedance"; label: "Low-impedance headphones"; detail: "Headphone output mode" }
                        Divider {}
                        Item {
                            width: parent.width; height: Math.max(defaultsButton.height, defaultsText.implicitHeight) + Style.space(16)
                            Column {
                                id: defaultsText; spacing: Style.space(2)
                                anchors.left: parent.left; anchors.right: defaultsButton.left; anchors.rightMargin: Style.space(12); anchors.verticalCenter: parent.verticalCenter
                                Label { width: parent.width; text: "System audio" }
                                Caption { width: parent.width; wrapMode: Text.WordWrap; text: "Use this mic and headphones in every app." }
                            }
                            Action { id: defaultsButton; anchors.right: parent.right; anchors.verticalCenter: parent.verticalCenter; text: "Set as default"; bordered: true; enabled: root.available; onClicked: root.send("defaults") }
                        }
                        Divider {}
                        Column {
                            width: parent.width; spacing: Style.space(6); topPadding: Style.space(8)
                            Caption { text: "ABOUT"; font.letterSpacing: 1 }
                            Caption { width: parent.width; wrapMode: Text.WordWrap; text: "These settings live on the device and stay after this panel closes. Lighting, Auto Gain and firmware updates need Elgato Wave Link; its app mixing and VST effects are separate." }
                        }
                    }
                    // Created only while the EQ page is shown, so its bindings cost nothing otherwise.
                    Loader {
                        id: eqPage; width: parent.width
                        active: root.available && root.page === "eq"; visible: active
                        sourceComponent: Column {
                            width: eqPage.width; spacing: Style.space(14)
                            Column {
                                width: parent.width; spacing: Style.space(4)
                                Rectangle {
                                    width: parent.width; height: Style.space(150); radius: Style.cornerRadius
                                    color: Util.alpha(root.fg, 0.04)
                                    EqGraph { anchors.fill: parent; editable: true }
                                    Caption { x: Style.space(6); y: Style.space(4); text: "+12"; font.pixelSize: Style.font.caption - 1 }
                                    Caption { anchors.right: parent.right; anchors.rightMargin: Style.space(6); anchors.bottom: parent.bottom; anchors.bottomMargin: Style.space(3); text: "−12 dB"; font.pixelSize: Style.font.caption - 1 }
                                }
                                Item {
                                    width: parent.width; height: Style.font.caption + Style.space(4)
                                    Repeater {
                                        model: [[20, "20"], [100, "100"], [1000, "1k"], [10000, "10k"], [20000, "20k"]]
                                        Caption {
                                            text: modelData[1]
                                            x: Math.max(0, Math.min(parent.width - width, root.eqX(modelData[0], parent.width) - width / 2))
                                        }
                                    }
                                }
                            }
                            Rectangle {
                                width: parent.width; height: tabs.height + Style.space(6); radius: Style.cornerRadius
                                color: Util.alpha(root.fg, 0.04)
                                Row {
                                    id: tabs; x: Style.space(3); y: Style.space(3); width: parent.width - Style.space(6); spacing: Style.space(4)
                                    BandTab { number: 1 }
                                    BandTab { number: 2 }
                                    BandTab { number: 3 }
                                    BandTab { number: 4 }
                                }
                            }
                            // Rebuilt per band so its sliders never carry another band's drag state.
                            Repeater {
                                model: [root.eqBand]
                                EqBand { number: modelData }
                            }
                        }
                    }
                }
            }
        }
    }
}
