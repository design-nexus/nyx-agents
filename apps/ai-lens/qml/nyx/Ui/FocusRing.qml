import QtQuick

// The keyboard focus outline around its parent, shown only for keyboard focus.
Rectangle {
    property Item target: parent
    property real radius_: target && target.radius !== undefined ? target.radius + 2 : Style.radiusSmall + 2

    anchors.fill: target
    anchors.margins: -3
    radius: radius_
    color: "transparent"
    border.width: 2
    border.color: Style.focus
    visible: target && target.activeFocus && target.focusReason !== Qt.MouseFocusReason
    z: 10
}
