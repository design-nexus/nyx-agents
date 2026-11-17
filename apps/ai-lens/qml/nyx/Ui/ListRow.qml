import QtQuick

// A list row: icon, title and optional subtitle, trailing items at the right (the
// default property), hover tint, selected `mix(bg, accent, selection)`.
Rectangle {
    id: row

    property string icon: ""
    property bool iconRecolor: icon.endsWith("-symbolic")
    property int iconSize: 18
    property string title: ""
    property string subtitle: ""
    property bool selected: false
    property bool interactive: true
    property bool hovered: area.containsMouse
    property int focusReason: Qt.OtherFocusReason
    default property alias trailing: tail.data

    signal clicked
    signal rightClicked(real x, real y)

    implicitHeight: subtitle !== "" ? Math.max(Style.rowHeight, 46) : Style.rowHeight
    implicitWidth: 240
    radius: Style.radiusSmall
    color: selected ? Style.selected : (interactive && (hovered || activeFocus)) ? Style.rowHover : "transparent"
    activeFocusOnTab: interactive
    Accessible.role: Accessible.ListItem
    Accessible.name: subtitle !== "" ? title + ", " + subtitle : title
    Accessible.selectable: interactive
    Accessible.selected: selected
    Accessible.onPressAction: clicked()
    onActiveFocusChanged: if (activeFocus) focusReason = area.pressed ? Qt.MouseFocusReason : Qt.TabFocusReason

    Behavior on color {
        ColorAnimation {
            duration: Style.fast
        }
    }

    MouseArea {
        id: area
        anchors.fill: parent
        enabled: row.interactive
        hoverEnabled: true
        acceptedButtons: Qt.LeftButton | Qt.RightButton
        cursorShape: row.interactive ? Qt.PointingHandCursor : Qt.ArrowCursor
        onClicked: m => m.button === Qt.RightButton ? row.rightClicked(m.x, m.y) : row.clicked()
    }

    Icon {
        id: ic
        visible: row.icon !== ""
        anchors.left: parent.left
        anchors.leftMargin: 10
        anchors.verticalCenter: parent.verticalCenter
        name: row.icon
        size: row.iconSize
        recolor: row.iconRecolor
        color: row.selected ? Style.accent : Style.dimText
    }

    Column {
        anchors.left: ic.visible ? ic.right : parent.left
        anchors.leftMargin: 10
        anchors.right: tail.left
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        spacing: 1
        Label {
            width: parent.width
            text: row.title
            font.weight: Font.Medium
        }
        Label {
            width: parent.width
            visible: row.subtitle !== ""
            text: row.subtitle
            role: "secondary"
            dim: true
        }
    }

    Row {
        id: tail
        anchors.right: parent.right
        anchors.rightMargin: 8
        anchors.verticalCenter: parent.verticalCenter
        spacing: 6
    }

    Keys.onPressed: e => {
        if (e.key === Qt.Key_Space || e.key === Qt.Key_Return || e.key === Qt.Key_Enter) {
            row.clicked();
            e.accepted = true;
        } else if (e.key === Qt.Key_Menu) {
            row.rightClicked(row.width / 2, row.height / 2);
            e.accepted = true;
        }
    }

    FocusRing {}
}
