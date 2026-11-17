import QtQuick

// A switch: a 36 × 20 pill; off `track`, on the accent with an `on_accent` knob.
// `toggled(on)` reports what the user asked; `checked` follows the backend.
Rectangle {
    id: t

    property bool checked: false
    property int focusReason: Qt.OtherFocusReason

    signal toggled(bool on)

    implicitWidth: 36
    implicitHeight: 20
    radius: height / 2
    color: checked ? Style.accent : Style.track
    opacity: enabled ? 1 : 0.45
    activeFocusOnTab: true
    Accessible.role: Accessible.CheckBox
    Accessible.checkable: true
    Accessible.checked: checked
    Accessible.onToggleAction: toggled(!checked)
    Accessible.onPressAction: toggled(!checked)
    onActiveFocusChanged: if (activeFocus) focusReason = area.pressed ? Qt.MouseFocusReason : Qt.TabFocusReason

    Behavior on color {
        ColorAnimation {
            duration: Style.normal
        }
    }

    Rectangle {
        width: 16
        height: 16
        radius: 8
        y: 2
        x: t.checked ? t.width - width - 2 : 2
        color: t.checked ? Style.onAccent : Style.text
        Behavior on x {
            NumberAnimation {
                duration: Style.normal
                easing.type: Easing.OutCubic
            }
        }
    }

    MouseArea {
        id: area
        anchors.fill: parent
        cursorShape: Qt.PointingHandCursor
        onClicked: t.toggled(!t.checked)
    }

    Keys.onPressed: e => {
        if (e.key === Qt.Key_Space || e.key === Qt.Key_Return || e.key === Qt.Key_Enter) {
            t.toggled(!t.checked);
            e.accepted = true;
        }
    }

    FocusRing {}
}
