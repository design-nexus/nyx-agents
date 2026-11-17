import QtQuick
import Quickshell

// A context or button menu in a popup: entries with icons, key hints, check marks
// and separators, the way nyx_ui::menu draws them.
//
//   Menu { id: m; anchorItem: someItem; entries: [{id: "pin", label: "Keep in Dock",
//          icon: "view-pin-symbolic"}, {separator: true}, {id: "quit", label: "Quit",
//          danger: true}]; onTriggered: id => … }
//   m.openAt(x, y)          // in anchorItem's coordinates
//
// Up and Down move, Enter picks, Escape or a click outside closes. Entry fields: id,
// label, icon, keys, checked (true/false: a check item), enabled, danger, separator,
// heading (a small label).
PopupWindow {
    id: menu

    property Item anchorItem: null
    property var entries: []
    property int current: -1
    property int minWidth: 200
    readonly property bool keyboardOff: Quickshell.env("NYX_SHELL_NO_KEYBOARD") === "1"

    signal triggered(string id)
    signal dismissed

    function openAt(x, y) {
        anchor.rect.x = x;
        anchor.rect.y = y;
        current = -1;
        visible = true;
        list.forceActiveFocus();
    }
    function close() {
        if (visible) {
            visible = false;
            dismissed();
        }
    }
    function pick(i) {
        const e = entries[i];
        if (!e || e.separator || e.heading || e.enabled === false)
            return;
        visible = false;
        triggered(e.id);
    }
    function move(by) {
        let i = current;
        for (let n = 0; n < entries.length; n++) {
            i = (i + by + entries.length) % entries.length;
            const e = entries[i];
            if (e && !e.separator && !e.heading && e.enabled !== false) {
                current = i;
                return;
            }
        }
    }

    anchor.item: anchorItem
    anchor.edges: Edges.Top | Edges.Left
    anchor.gravity: Edges.Bottom | Edges.Right
    anchor.adjustment: PopupAdjustment.All
    grabFocus: !keyboardOff
    color: "transparent"
    visible: false
    implicitWidth: Math.max(minWidth, col.implicitWidth) + 12
    implicitHeight: col.implicitHeight + 12
    onVisibleChanged: if (!visible) dismissed()

    Surface {
        anchors.fill: parent
        shadow: false

        FocusScope {
            id: list
            anchors.fill: parent
            anchors.margins: 6
            focus: true
            Keys.onUpPressed: menu.move(-1)
            Keys.onDownPressed: menu.move(1)
            Keys.onReturnPressed: menu.pick(menu.current)
            Keys.onEnterPressed: menu.pick(menu.current)
            Keys.onEscapePressed: menu.close()

            Column {
                id: col
                width: parent.width
                Repeater {
                    model: menu.entries
                    delegate: Item {
                        id: entry
                        required property var modelData
                        required property int index
                        readonly property bool sep: !!modelData.separator
                        readonly property bool head: !!modelData.heading
                        readonly property bool on: modelData.enabled !== false
                        width: col.width
                        implicitWidth: rowContent.implicitWidth + 24
                        height: sep ? 9 : head ? 26 : 32

                        Separator {
                            visible: entry.sep
                            anchors.verticalCenter: parent.verticalCenter
                            width: parent.width
                        }
                        Label {
                            visible: entry.head
                            anchors.left: parent.left
                            anchors.leftMargin: 10
                            anchors.verticalCenter: parent.verticalCenter
                            text: entry.modelData.heading || ""
                            role: "group"
                            color: Style.tertiary
                        }
                        Rectangle {
                            visible: !entry.sep && !entry.head
                            anchors.fill: parent
                            radius: Style.radiusSmall
                            color: (menu.current === entry.index || area.containsMouse) && entry.on ? (entry.modelData.danger ? Style.alpha(Style.danger, 0.16) : Style.rowHover) : "transparent"
                            opacity: entry.on ? 1 : 0.45

                            Row {
                                id: rowContent
                                anchors.left: parent.left
                                anchors.leftMargin: 10
                                anchors.verticalCenter: parent.verticalCenter
                                spacing: 10
                                Item {
                                    width: 16
                                    height: 16
                                    anchors.verticalCenter: parent.verticalCenter
                                    visible: entry.modelData.icon !== undefined || entry.modelData.checked !== undefined
                                    Icon {
                                        anchors.fill: parent
                                        name: entry.modelData.checked === true ? "object-select-symbolic" : (entry.modelData.icon || "")
                                        visible: name !== ""
                                        size: 16
                                        recolor: entry.modelData.checked !== undefined || name.endsWith("-symbolic")
                                        color: entry.modelData.checked === true ? Style.accent : entry.modelData.danger ? Style.danger : Style.dimText
                                    }
                                }
                                Label {
                                    anchors.verticalCenter: parent.verticalCenter
                                    text: entry.modelData.label || ""
                                    color: entry.modelData.danger ? Style.danger : Style.text
                                }
                            }
                            Label {
                                anchors.right: parent.right
                                anchors.rightMargin: 10
                                anchors.verticalCenter: parent.verticalCenter
                                visible: !!entry.modelData.keys
                                text: entry.modelData.keys || ""
                                role: "caption"
                                color: Style.tertiary
                            }
                            MouseArea {
                                id: area
                                anchors.fill: parent
                                hoverEnabled: true
                                enabled: entry.on
                                cursorShape: Qt.PointingHandCursor
                                onClicked: menu.pick(entry.index)
                            }
                        }
                    }
                }
            }
        }
    }
}
