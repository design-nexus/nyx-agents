import QtQuick

// A split-action control (Control Center's Wi-Fi, Bluetooth, VPN): the main part acts
// at once (a switch), the arrow opens details. Active: accent tint, icon in the
// accent. `compact` drops the text and the arrow (a 1×1 tile).
Rectangle {
    id: t

    property string icon: ""
    property string title: ""
    property string subtitle: ""
    property bool active: false
    property bool busy: false
    property bool hasDetails: true
    property bool compact: false
    property bool hovered: main.containsMouse || arrow.containsMouse
    property int focusReason: Qt.OtherFocusReason

    signal clicked
    signal detailsClicked
    signal rightClicked

    radius: Style.radiusCard
    color: active ? Style.liftActive : Style.card
    border.width: Style.blended ? 0 : 1
    border.color: Style.alpha(Style.border, 0.6)
    activeFocusOnTab: true
    implicitHeight: 48
    implicitWidth: compact ? 48 : 160

    Rectangle {
        id: lead
        anchors.left: parent.left
        anchors.leftMargin: t.compact ? (t.width - width) / 2 : 8
        anchors.verticalCenter: parent.verticalCenter
        width: 32
        height: 32
        radius: Style.radiusSmall
        color: t.active ? Style.accent : main.containsMouse ? Style.rowHover : Style.field
        Icon {
            anchors.centerIn: parent
            name: t.icon
            size: 16
            recolor: true
            color: t.active ? Style.onAccent : Style.text
        }
        Spinner {
            anchors.centerIn: parent
            visible: t.busy
            size: 30
            color: t.active ? Style.onAccent : Style.accent
        }
    }

    Column {
        visible: !t.compact
        anchors.left: lead.right
        anchors.leftMargin: 10
        anchors.right: arrowBox.visible ? arrowBox.left : parent.right
        anchors.rightMargin: 4
        anchors.verticalCenter: parent.verticalCenter
        Label {
            width: parent.width
            text: t.title
            font.weight: Font.Medium
        }
        Label {
            width: parent.width
            visible: t.subtitle !== ""
            text: t.subtitle
            role: "caption"
            dim: true
        }
    }

    MouseArea {
        id: main
        anchors.left: parent.left
        anchors.top: parent.top
        anchors.bottom: parent.bottom
        anchors.right: arrowBox.visible ? arrowBox.left : parent.right
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        cursorShape: Qt.PointingHandCursor
        onClicked: m => m.button === Qt.RightButton ? t.rightClicked() : t.clicked()
    }

    Rectangle {
        id: arrowBox
        visible: t.hasDetails && !t.compact
        anchors.right: parent.right
        anchors.rightMargin: 6
        anchors.verticalCenter: parent.verticalCenter
        width: 28
        height: 32
        radius: Style.radiusSmall
        color: arrow.containsMouse ? Style.rowHover : "transparent"
        Icon {
            anchors.centerIn: parent
            name: "go-next-symbolic"
            size: 14
            recolor: true
            color: Style.dimText
        }
        MouseArea {
            id: arrow
            anchors.fill: parent
            hoverEnabled: true
            cursorShape: Qt.PointingHandCursor
            onClicked: t.detailsClicked()
        }
    }

    Keys.onPressed: e => {
        if (e.key === Qt.Key_Space || e.key === Qt.Key_Return || e.key === Qt.Key_Enter) {
            t.clicked();
            e.accepted = true;
        } else if (e.key === Qt.Key_Right && t.hasDetails) {
            t.detailsClicked();
            e.accepted = true;
        }
    }

    FocusRing {}
}
