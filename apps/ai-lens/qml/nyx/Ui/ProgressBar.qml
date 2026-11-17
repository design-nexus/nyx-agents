import QtQuick

// A 3–6 px meter in `track` with an accent fill (`value` 0–1).
Rectangle {
    id: p

    property real value: 0
    property color fill: Style.accent

    implicitHeight: 4
    implicitWidth: 120
    radius: height / 2
    color: Style.track

    Rectangle {
        height: parent.height
        radius: parent.radius
        width: parent.width * Math.max(0, Math.min(1, p.value))
        color: p.fill
        Behavior on width {
            NumberAnimation {
                duration: Style.normal
            }
        }
    }
}
