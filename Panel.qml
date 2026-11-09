import QtQuick
import QtQuick.Controls as Controls
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
    function openPage(name) {
        page = name; scroll.contentY = 0
        Qt.callLater(function() { (name === "eq" ? eqBackButton : backButton).forceActiveFocus() })
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
    function back() {
        var from = page
        phantomArmed = false; page = "controls"
        Qt.callLater(function() { (from === "eq" ? eqButton : hardwareButton).forceActiveFocus() })
    }
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
        property bool logarithmic: false
        property real logStep: 0.01
        property string quantize: ""
        property string display: ""
        property bool editable: true
        readonly property bool live: root.supported(controlKey) && editable
        readonly property real current: root.value(controlKey) !== undefined ? Number(root.value(controlKey)) : minimum
        width: parent.width; spacing: Style.space(3)
        Row {
            width: parent.width
            Label { width: parent.width - numeric.width; text: field.label; anchors.verticalCenter: parent.verticalCenter }
            Label {
                id: numeric
                text: root.value(field.controlKey) === undefined ? "—" : field.display !== "" ? field.display : field.controlKey === "balance" ? (field.current === 100 ? "Centered" : field.current < 100 ? "Mic +" + Math.round(100-field.current) : "Computer +" + Math.round(field.current-100)) : String(Math.round(field.current * 100) / 100) + field.unit
                font.pixelSize: field.hero ? Style.font.displayLarge : Style.font.body
                color: root.highlights[field.controlKey] ? Commons.Color.accent : root.fg
            }
        }
        Row {
            width: parent.width; spacing: Style.space(6)
            Action { visible: field.hero; width: Style.space(28); text: "−"; enabled: field.live; onClicked: field.adjust(-1, false) }
            PanelSlider {
                id: slider
                objectName: field.controlKey
                onActiveFocusChanged: if (activeFocus) { root.focusedControl = field.controlKey; root.ensureVisible(field) }
                width: parent.width - (field.hero ? Style.space(68) : 0)
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
            Action { visible: field.hero; width: Style.space(28); text: "+"; enabled: field.live; onClicked: field.adjust(1, false) }
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
            foreground: root.fg; accent: Commons.Color.accent
        }
    }
    component EqBand: Column {
        id: band
        property int number
        readonly property string prefix: "eq" + number
        readonly property int type: root.value(prefix + "Type") !== undefined ? Number(root.value(prefix + "Type")) : 2
        readonly property bool active: !!root.value(prefix + "On")
        width: parent.width; spacing: Style.space(8)
        Effect { controlKey: band.prefix + "On"; label: "Band " + band.number }
        Column {
            width: parent.width; spacing: Style.space(8)
            opacity: band.active ? 1 : 0.55
            Row {
                width: parent.width; spacing: Style.space(6)
                Label { width: parent.width - typePrev.width - typeName.width - typeNext.width - 3 * parent.spacing; text: "Filter"; anchors.verticalCenter: parent.verticalCenter }
                Action {
                    id: typePrev; width: Style.space(28); text: "‹"; objectName: band.prefix + "TypePrev"
                    Accessible.name: "Previous filter type, band " + band.number
                    enabled: root.supported(band.prefix + "Type") && !root.pending[band.prefix + "Type"]
                    onClicked: root.edit(band.prefix + "Type", (band.type + 5) % 6, true)
                }
                Label {
                    id: typeName; width: Style.space(92); horizontalAlignment: Text.AlignHCenter; anchors.verticalCenter: parent.verticalCenter
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
            SliderField {
                controlKey: band.prefix + "Freq"; label: "Frequency"; minimum: 20; maximum: 20000; increment: 1
                logarithmic: true; logStep: Math.log10(2) / 12; display: root.formatHz(current)
            }
            SliderField {
                controlKey: band.prefix + "Gain"; label: "Gain"; minimum: -12; maximum: 12; increment: 0.25; unit: " dB"
                editable: root.eqHasGain(band.type)
                display: editable ? "" : "n/a"
            }
            SliderField {
                controlKey: band.prefix + "Q"; label: "Q"; minimum: 0.1; maximum: 10; increment: 0.01
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
                            Row {
                                width: parent.width; spacing: Style.space(10)
                                DeviceIcon { id: headingIcon; width: Style.space(24); height: width; ink: root.fg; anchors.verticalCenter: parent.verticalCenter }
                                Label { width: parent.width - headingIcon.width - parent.spacing; wrapMode: Text.WordWrap; text: root.page === "controls" ? "Elgato Wave XLR MK.2" : root.page === "eq" ? "Equalizer" : "Hardware settings"; font.pixelSize: Style.font.heading; font.bold: true }
                            }
                            Caption { text: root.disconnected ? "Not connected" : root.stale ? "Reconnecting" : root.state.usb ? "Connected · USB" : "Connected · basic audio"; color: root.disconnected ? root.fg : root.stale ? Commons.Color.urgent : root.fg }
                        }
                        Action { id: helpButton; visible: !root.disconnected; text: "?"; tooltipText: "Controls and keyboard help"; bordered: true; onClicked: root.helpOpen = !root.helpOpen }
                    }
                    Caption {
                        width: parent.width; wrapMode: Text.WordWrap; visible: root.helpOpen
                        text: "M: mute mic. Tab: next control. ← →: adjust. Headphones: 0.25 dB per arrow key; wheel: 1 dB, Shift-wheel: 0.25 dB. Gain: 1 dB. Blend: 0 = mic, 100 = centered, 200 = computer. EQ: frequency steps 1/12 octave; Q steps match the device. These are settings, not signal meters. Hardware changes highlight briefly."
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
                        visible: root.available && root.page === "controls"; width: parent.width; spacing: Style.space(14)
                        Row {
                            width: parent.width
                            Caption { width: parent.width - muteButton.width; text: "MICROPHONE"; anchors.verticalCenter: parent.verticalCenter; font.letterSpacing: 1 }
                            Action { id: muteButton; text: root.value("mute") ? "Unmute mic  M" : "Mute mic  M"; foreground: root.value("mute") ? Commons.Color.urgent : root.fg; bordered: true; enabled: root.supported("mute") && !root.pending.mute; onClicked: root.toggleSetting("mute") }
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
                        Action {
                            id: eqButton; width: parent.width; leftAlign: true; visible: root.state.eq !== undefined
                            text: "Equalizer  ·  " + (root.value("eq") ? "On" : "Off") + "  →"
                            onClicked: root.openPage("eq")
                        }
                        Caption {
                            width: parent.width; wrapMode: Text.WordWrap
                            visible: root.state.usb === true && root.state.eq === undefined
                            text: root.eqUnavailableText()
                        }
                        Divider {}
                        Action {
                            id: hardwareButton; width: parent.width; leftAlign: true
                            text: "Hardware settings  ·  48V " + (root.state.phantom === undefined ? "—" : root.state.phantom ? "On" : "Off") + "  →"
                            onClicked: root.openPage("hardware")
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
                            Label { width: parent.width; wrapMode: Text.WordWrap; text: "Turn 48V " + (root.phantomTarget ? "on" : "off") + "? Use phantom power only when your microphone requires it."; color: Commons.Color.urgent }
                            Row {
                                spacing: Style.space(8)
                                Action { id: cancelPhantom; text: "Cancel"; bordered: true; onClicked: { root.phantomArmed = false; phantomButton.forceActiveFocus() } }
                                Action { text: "Turn " + (root.phantomTarget ? "on" : "off"); bordered: true; foreground: Commons.Color.urgent; enabled: root.supported("phantom"); onClicked: { root.edit("phantom", root.phantomTarget ? 1 : 0, true); root.phantomArmed = false; phantomButton.forceActiveFocus() } }
                            }
                        }
                        Divider {}
                        Effect { controlKey: "lowImpedance"; label: "Low impedance mode" }
                        Caption { text: "Headphone output" }
                        Divider {}
                        Caption { text: "ABOUT THIS CONTROL PANEL"; font.letterSpacing: 1 }
                        Caption { visible: root.state.firmware !== undefined; text: "Firmware " + root.state.firmware }
                        Caption { width: parent.width; wrapMode: Text.WordWrap; text: "Controls the device’s onboard settings, including its hardware EQ. Lighting, Auto Gain and firmware updates aren’t available here. Wave Link app mixing and VST effects are separate." }
                        Action { text: "Use mic + headphones as defaults"; bordered: true; enabled: root.available; onClicked: root.send("defaults") }
                        Action { id: backButton; text: "← Back to controls"; foreground: Commons.Color.accent; onClicked: root.back() }
                    }
                    Column {
                        visible: root.available && root.page === "eq"; width: parent.width; spacing: Style.space(14)
                        Effect { controlKey: "eq"; label: "Equalizer" }
                        Caption { width: parent.width; wrapMode: Text.WordWrap; text: "Runs on the device’s own processor, so it shapes the mic for every app and stays on after this panel closes. Bands set in Wave Link are kept." }
                        Divider {}
                        Column {
                            width: parent.width; spacing: Style.space(14)
                            opacity: root.value("eq") ? 1 : 0.7
                            EqBand { number: 1 }
                            Divider {}
                            EqBand { number: 2 }
                            Divider {}
                            EqBand { number: 3 }
                            Divider {}
                            EqBand { number: 4 }
                        }
                        Divider {}
                        Action { id: eqBackButton; text: "← Back to controls"; foreground: Commons.Color.accent; onClicked: root.back() }
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
