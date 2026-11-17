import QtQuick
import "nyx/Ui" as Nyx
import "Model.js" as Model

Card {
    id: root
    property var session: ({})
    property string agentName: ""
    property color identity: Nyx.Style.accent
    live: !!session.live
    interactive: true
    Plain {
        width: parent.width
        text: root.session.title || "Untitled session"
        strong: true
    }
    Row {
        width: parent.width
        spacing: 7
        Plain {
            width: Math.min(170, implicitWidth)
            text: root.agentName
            color: root.identity
            role: "caption"
            strong: true
        }
        Plain {
            width: parent.width - Math.min(170, root.agentName.length * 7) - 7
            text: Model.shortPath(root.session.cwd) + (root.session.branch ? " · " + root.session.branch : "")
            dim: true
            role: "caption"
        }
    }
    Row {
        width: parent.width
        Plain {
            width: parent.width * .52
            text: root.session.status || "Unknown"
            role: "caption"
            color: Model.need(root.session) ? Nyx.Style.warning : root.session.live ? Nyx.Style.accent : Nyx.Style.dimText
        }
        Plain {
            width: parent.width * .48
            text: Model.time(root.session.updated)
            role: "caption"
            dim: true
            horizontalAlignment: Text.AlignRight
            wrapMode: Text.NoWrap
            maximumLineCount: 1
        }
    }
    Plain {
        width: parent.width
        visible: Model.need(root.session) && text !== ""
        text: root.session.message || "User input requested"
        role: "caption"
        color: Nyx.Style.warning
    }
}
