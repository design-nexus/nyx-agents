import QtQuick
import QtQuick.Controls as Controls
import "nyx/Ui" as Nyx

Row {
    id: root
    property string label: ""
    property real remaining: -1
    property string reset: ""
    property color tone: Nyx.Style.accent
    spacing: 8
    height: 15
    Plain {
        width: 100
        height: 15
        text: root.label
        size: 11
        color: root.tone
        wrapMode: Text.NoWrap
        maximumLineCount: 1
    }
    Item {
        width: Math.max(0, root.width - 168)
        height: 15
        Rectangle {
            anchors.verticalCenter: parent.verticalCenter
            width: parent.width
            height: 4
            radius: 2
            color: Nyx.Style.alpha(Nyx.Style.text, .1)
            Rectangle {
                height: parent.height
                width: parent.width * Math.max(0, Math.min(100, root.remaining)) / 100
                radius: 2
                color: root.tone
            }
        }
    }
    Plain {
        width: 52
        height: 15
        text: root.remaining < 0 ? "—" : Math.round(root.remaining) + "% left"
        size: 11
        horizontalAlignment: Text.AlignRight
        wrapMode: Text.NoWrap
        maximumLineCount: 1
    }
    HoverHandler {
        id: hover
    }
    Controls.ToolTip {
        id: tip
        visible: hover.hovered && root.reset !== ""
        delay: 400
        text: root.reset
        contentItem: Plain {
            text: tip.text
            role: "caption"
        }
        background: Rectangle {
            color: Nyx.Style.popover
            radius: Nyx.Style.radiusSmall
            border.color: Nyx.Style.border
        }
    }
}
