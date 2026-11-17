import QtQuick

// Activity: a turning arc, still under Reduce motion.
Item {
    id: s

    property int size: 16
    property color color: Style.dimText
    property bool running: visible

    implicitWidth: size
    implicitHeight: size

    Canvas {
        id: arc
        anchors.fill: parent
        onPaint: {
            const ctx = getContext("2d");
            ctx.reset();
            ctx.lineWidth = Math.max(1.5, s.size / 9);
            ctx.lineCap = "round";
            ctx.strokeStyle = s.color;
            ctx.beginPath();
            ctx.arc(width / 2, height / 2, width / 2 - ctx.lineWidth, 0, Math.PI * 1.4);
            ctx.stroke();
        }
        RotationAnimator on rotation {
            from: 0
            to: 360
            duration: 900
            loops: Animation.Infinite
            running: s.running && !Style.reducedMotion
        }
    }
    onColorChanged: arc.requestPaint()
}
