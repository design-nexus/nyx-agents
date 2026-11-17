import QtQuick

// A 32 px square icon button: transparent, `dim_text` icon, hover tint and `text`;
// toggled: accent tint with the icon in the accent; primary: solid accent. Space and
// Enter press it when focused.
Rectangle {
    id: b

    property string icon: ""
    property int iconSize: 16
    property bool toggled: false
    property bool primary: false
    property bool danger: false
    property string tooltip: ""
    property bool hovered: area.containsMouse
    property alias pressed: area.pressed
    property int focusReason: Qt.OtherFocusReason

    signal clicked
    signal rightClicked

    implicitWidth: Style.iconButton
    implicitHeight: Style.iconButton
    radius: Style.radiusSmall
    activeFocusOnTab: true
    Accessible.role: Accessible.Button
    Accessible.name: tooltip
    Accessible.checkable: toggled
    Accessible.checked: toggled
    Accessible.onPressAction: clicked()
    onActiveFocusChanged: if (activeFocus) focusReason = area.pressed ? Qt.MouseFocusReason : Qt.TabFocusReason
    color: primary ? Style.accent : toggled ? Style.alpha(Style.accent, 0.18) : (hovered || activeFocus) ? (danger ? Style.alpha(Style.danger, 0.16) : Style.rowHover) : "transparent"
    opacity: enabled ? 1 : 0.45

    Behavior on color {
        ColorAnimation {
            duration: Style.fast
        }
    }

    Icon {
        anchors.centerIn: parent
        name: b.icon
        size: b.iconSize
        recolor: true
        color: b.primary ? Style.onAccent : b.toggled ? Style.accent : (b.danger && b.hovered) ? Style.danger : b.hovered ? Style.text : Style.dimText
    }

    MouseArea {
        id: area
        anchors.fill: parent
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        cursorShape: Qt.PointingHandCursor
        onClicked: m => m.button === Qt.RightButton ? b.rightClicked() : b.clicked()
    }

    Keys.onPressed: e => {
        if (e.key === Qt.Key_Space || e.key === Qt.Key_Return || e.key === Qt.Key_Enter) {
            b.clicked();
            e.accepted = true;
        }
    }

    Tip {
        text: b.tooltip
        shown: b.hovered && b.tooltip !== ""
    }

    FocusRing {}
}
