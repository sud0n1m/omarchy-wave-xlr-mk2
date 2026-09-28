import QtQuick

// Original device silhouette shared by the bar and panel heading.
Canvas {
    id: deviceIcon
    property color ink: "white"
    property bool muted: false
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
