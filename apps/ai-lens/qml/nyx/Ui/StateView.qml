import QtQuick

// Empty, loading and error states: an icon (or a spinner), one line of what's going
// on and, if useful, one action, centred.
Item {
    id: v

    property string kind: "empty"
    property string icon: kind === "error" ? "dialog-warning-symbolic" : "action-unavailable-symbolic"
    property string text: ""
    property string actionText: ""

    signal action

    implicitHeight: col.implicitHeight + 24
    implicitWidth: 240

    Column {
        id: col
        anchors.centerIn: parent
        width: Math.min(parent.width - 24, 320)
        spacing: 10
        Spinner {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: v.kind === "loading"
            size: 22
        }
        Icon {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: v.kind !== "loading" && v.icon !== ""
            name: v.icon
            size: 28
            recolor: true
            color: v.kind === "error" ? Style.warning : Style.tertiary
        }
        Label {
            width: parent.width
            horizontalAlignment: Text.AlignHCenter
            wrapMode: Text.Wrap
            text: v.text
            dim: true
        }
        Button {
            anchors.horizontalCenter: parent.horizontalCenter
            visible: v.actionText !== ""
            text: v.actionText
            onClicked: v.action()
        }
    }
}
