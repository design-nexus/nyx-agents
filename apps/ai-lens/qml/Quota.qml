import QtQuick
import "nyx/Ui" as Nyx

Column {
    property string label: ""
    property real remaining: -1
    property string reset: ""
    property color tone: Nyx.Style.accent
    spacing: 4
    Row {
        width: parent.width
        Plain {
            width: parent.width * .62
            text: parent.parent.label
            role: "caption"
        }
        Plain {
            width: parent.width * .38
            text: parent.parent.remaining < 0 ? "Unavailable" : Math.round(parent.parent.remaining) + "% left"
            horizontalAlignment: Text.AlignRight
            role: "caption"
        }
    }
    Rectangle {
        width: parent.width
        height: 4
        radius: 2
        color: Nyx.Style.alpha(Nyx.Style.text, .1)
        Rectangle {
            height: parent.height
            width: parent.width * Math.max(0, Math.min(100, parent.parent.remaining)) / 100
            radius: 2
            color: parent.parent.tone
        }
    }
    Plain {
        width: parent.width
        text: parent.reset
        wrapMode: Text.NoWrap
        maximumLineCount: 1
        dim: true
        role: "caption"
        visible: text !== ""
    }
}
