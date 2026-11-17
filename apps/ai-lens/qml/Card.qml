import QtQuick
import "nyx/Ui" as Nyx

Rectangle {
    id: root
    default property alias content: col.data
    property real paddingX: 12
    property real paddingY: 11
    property real contentSpacing: 7
    property color accent: "transparent"
    property bool live: false
    property bool interactive: false
    readonly property bool hovered: mouse.containsMouse
    signal clicked
    implicitHeight: col.implicitHeight + paddingY * 2
    color: hovered && interactive ? Nyx.Style.rowHover : accent.a > 0 ? Nyx.Style.mix(Nyx.Style.card, accent, .045) : Nyx.Style.card
    radius: Nyx.Style.radiusCard
    border.width: live || accent.a > 0 ? 1 : 0
    border.color: live ? Nyx.Style.alpha(Nyx.Style.accent, .5) : Nyx.Style.alpha(accent, .25)
    activeFocusOnTab: interactive
    Accessible.role: interactive ? Accessible.Button : Accessible.Grouping
    Accessible.onPressAction: clicked()
    data: [
        Column {
            id: col
            x: root.paddingX
            y: root.paddingY
            width: root.width - root.paddingX * 2
            spacing: root.contentSpacing
        },
        MouseArea {
            id: mouse
            anchors.fill: root
            enabled: root.interactive
            hoverEnabled: true
            onClicked: root.clicked()
            cursorShape: Qt.PointingHandCursor
        },
        Nyx.FocusRing {
            target: root
            visible: root.activeFocus
        }
    ]
    Keys.onReturnPressed: if (interactive)
        clicked()
    Keys.onSpacePressed: if (interactive)
        clicked()
}
