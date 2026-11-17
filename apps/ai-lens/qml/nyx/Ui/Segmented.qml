import QtQuick

// A segmented control: a `field` track, 2 px inside; the selected segment is
// `@theme_raised`. `options` is a list of {id, label, icon?}; `chosen(id)` reports a
// click, `current` follows the backend. Left and Right move between segments.
Rectangle {
    id: seg

    property var options: []
    property string current: ""
    property int focusReason: Qt.OtherFocusReason

    signal chosen(string id)

    implicitHeight: Style.controlHeight
    implicitWidth: row.implicitWidth + 4
    radius: Style.radiusSmall
    color: Style.field
    activeFocusOnTab: true
    Accessible.role: Accessible.PageTabList

    Row {
        id: row
        anchors.fill: parent
        anchors.margins: 2
        spacing: 2
        Repeater {
            model: seg.options
            delegate: Rectangle {
                id: opt
                required property var modelData
                readonly property bool on: modelData.id === seg.current
                Accessible.role: Accessible.PageTab
                Accessible.name: modelData.label || ""
                Accessible.selected: on
                Accessible.onPressAction: seg.chosen(modelData.id)
                height: row.height
                width: Math.max(56, (seg.width - 4 - (seg.options.length - 1) * 2) / Math.max(1, seg.options.length))
                radius: Style.radiusSmall - 2
                color: on ? Style.raised : area.containsMouse ? Style.rowHover : "transparent"
                Row {
                    anchors.centerIn: parent
                    spacing: 5
                    Icon {
                        anchors.verticalCenter: parent.verticalCenter
                        visible: !!opt.modelData.icon
                        name: opt.modelData.icon || ""
                        size: 14
                        recolor: true
                        color: opt.on ? Style.text : Style.dimText
                    }
                    Label {
                        anchors.verticalCenter: parent.verticalCenter
                        text: opt.modelData.label || ""
                        role: "secondary"
                        strong: opt.on
                        color: opt.on ? Style.text : Style.dimText
                    }
                }
                MouseArea {
                    id: area
                    anchors.fill: parent
                    hoverEnabled: true
                    cursorShape: Qt.PointingHandCursor
                    onClicked: seg.chosen(opt.modelData.id)
                }
            }
        }
    }

    Keys.onPressed: e => {
        const i = options.findIndex(o => o.id === current);
        if (e.key === Qt.Key_Left && i > 0)
            chosen(options[i - 1].id);
        else if (e.key === Qt.Key_Right && i < options.length - 1)
            chosen(options[i + 1].id);
        else
            return;
        e.accepted = true;
    }

    FocusRing {}
}
