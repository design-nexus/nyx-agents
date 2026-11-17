import QtQuick

// A text button (optionally with an icon). `primary` is the solid accent button (one
// per view); `toggled` is an accent tint, for the selected one of a row of tabs;
// `danger` arms on the first click and acts on a second within 4 s, as STYLE.md asks
// of destructive actions, unless `confirm` is false.
Rectangle {
    id: b

    property string text: ""
    property string icon: ""
    property bool primary: false
    property bool toggled: false
    property bool danger: false
    property bool confirm: danger
    property string armedText: "Click again"
    property bool flat: false
    property bool armed: false
    property bool hovered: area.containsMouse
    property int focusReason: Qt.OtherFocusReason

    signal clicked

    function press() {
        if (!enabled)
            return;
        if (confirm && !armed) {
            armed = true;
            disarm.restart();
            return;
        }
        armed = false;
        clicked();
    }

    implicitHeight: Style.controlHeight
    implicitWidth: row.implicitWidth + 24
    radius: Style.radiusSmall
    activeFocusOnTab: true
    // For screen readers: a button named by its text; pressing it as Space would.
    Accessible.role: Accessible.Button
    Accessible.name: armed ? armedText : text
    Accessible.checkable: toggled
    Accessible.checked: toggled
    Accessible.onPressAction: press()
    onActiveFocusChanged: if (activeFocus) focusReason = area.pressed ? Qt.MouseFocusReason : Qt.TabFocusReason
    opacity: enabled ? 1 : 0.45
    color: primary ? (hovered ? Qt.lighter(Style.accent, 1.08) : Style.accent) : armed ? Style.alpha(Style.danger, 0.2) : (danger && hovered) ? Style.alpha(Style.danger, 0.14) : toggled ? Style.alpha(Style.accent, hovered ? 0.26 : 0.2) : hovered ? Style.rowHover : flat ? "transparent" : Style.field
    border.width: primary || toggled || flat ? 0 : 1
    border.color: Style.alpha(Style.border, 0.5)

    Behavior on color {
        ColorAnimation {
            duration: Style.fast
        }
    }

    Timer {
        id: disarm
        interval: 4000
        onTriggered: b.armed = false
    }

    Row {
        id: row
        anchors.centerIn: parent
        spacing: 6
        Icon {
            anchors.verticalCenter: parent.verticalCenter
            visible: b.icon !== ""
            name: b.icon
            size: 16
            recolor: true
            color: label.color
        }
        Label {
            id: label
            anchors.verticalCenter: parent.verticalCenter
            text: b.armed ? b.armedText : b.text
            strong: true
            color: b.primary ? Style.onAccent : (b.danger && (b.hovered || b.armed)) ? Style.danger : Style.text
        }
    }

    MouseArea {
        id: area
        anchors.fill: parent
        hoverEnabled: true
        cursorShape: Qt.PointingHandCursor
        onClicked: b.press()
    }

    Keys.onPressed: e => {
        if (e.key === Qt.Key_Space || e.key === Qt.Key_Return || e.key === Qt.Key_Enter) {
            b.press();
            e.accepted = true;
        }
    }

    FocusRing {}
}
