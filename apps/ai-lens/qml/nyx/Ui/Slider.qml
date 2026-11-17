import QtQuick

// A horizontal slider: a `track` bar with an accent fill and a knob, an optional icon
// button at the left (mute, outputs). `moved(value)` fires while dragging (throttle
// in the backend); `released(value)` once at the end. Arrow keys step, the wheel
// steps, Home and End go to the ends.
Item {
    id: s

    property real from: 0
    property real to: 100
    property real step: 1
    property real value: 0
    property string icon: ""
    property bool iconToggled: false
    property string iconTooltip: ""
    property int trackHeight: 6
    property bool dragging: area.pressed
    readonly property real shown: dragging ? live : value
    property real live: value
    property int focusReason: Qt.OtherFocusReason

    signal moved(real value)
    signal released(real value)
    signal iconClicked

    function clamp(v) {
        const stepped = step > 0 ? Math.round((v - from) / step) * step + from : v;
        return Math.max(from, Math.min(to, stepped));
    }
    function nudge(by) {
        const v = clamp(value + by);
        live = v;
        moved(v);
        released(v);
    }

    implicitHeight: 32
    implicitWidth: 200
    activeFocusOnTab: true
    Accessible.role: Accessible.Slider
    Accessible.name: iconTooltip
    Accessible.description: Math.round(live) + ""
    Accessible.onIncreaseAction: nudge(Math.max(step, (to - from) / 20))
    Accessible.onDecreaseAction: nudge(-Math.max(step, (to - from) / 20))
    onActiveFocusChanged: if (activeFocus) focusReason = area.pressed ? Qt.MouseFocusReason : Qt.TabFocusReason

    IconButton {
        id: iconButton
        visible: s.icon !== ""
        anchors.left: parent.left
        anchors.verticalCenter: parent.verticalCenter
        icon: s.icon
        toggled: s.iconToggled
        tooltip: s.iconTooltip
        activeFocusOnTab: false
        onClicked: s.iconClicked()
    }

    Item {
        id: bar
        anchors.left: iconButton.visible ? iconButton.right : parent.left
        anchors.leftMargin: iconButton.visible ? 6 : 0
        anchors.right: parent.right
        anchors.verticalCenter: parent.verticalCenter
        height: 20

        readonly property real fraction: s.to > s.from ? (s.shown - s.from) / (s.to - s.from) : 0

        Rectangle {
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width
            height: s.trackHeight
            radius: height / 2
            color: Style.track
            Rectangle {
                height: parent.height
                radius: parent.radius
                width: Math.max(height, parent.width * bar.fraction)
                color: Style.accent
            }
        }
        Rectangle {
            width: 16
            height: 16
            radius: 8
            anchors.verticalCenter: parent.verticalCenter
            x: Math.max(0, Math.min(parent.width - width, parent.width * bar.fraction - width / 2))
            color: Style.text
            border.width: 1
            border.color: Style.alpha(Style.shadow, 0.3)
        }

        MouseArea {
            id: area
            anchors.fill: parent
            anchors.margins: -6
            cursorShape: Qt.PointingHandCursor
            function at(mx) {
                return s.clamp(s.from + (s.to - s.from) * Math.max(0, Math.min(1, (mx - 6) / bar.width)));
            }
            onPressed: m => {
                s.live = at(m.x);
                s.moved(s.live);
            }
            onPositionChanged: m => {
                if (pressed) {
                    s.live = at(m.x);
                    s.moved(s.live);
                }
            }
            onReleased: s.released(s.live)
            onWheel: w => s.nudge((w.angleDelta.y + w.angleDelta.x > 0 ? 1 : -1) * Math.max(s.step, (s.to - s.from) / 20))
        }
    }

    Keys.onPressed: e => {
        const big = Math.max(step, (to - from) / 10);
        if (e.key === Qt.Key_Left || e.key === Qt.Key_Down)
            nudge(-big / 2);
        else if (e.key === Qt.Key_Right || e.key === Qt.Key_Up)
            nudge(big / 2);
        else if (e.key === Qt.Key_Home)
            nudge(from - value);
        else if (e.key === Qt.Key_End)
            nudge(to - value);
        else
            return;
        e.accepted = true;
    }

    FocusRing {}
}
