import QtQuick
import Quickshell

// A tooltip for its parent item, in a popup so it can leave the surface (a dock's
// icons): shown after a short rest, above the item unless `below`. It never takes the
// pointer (an empty input region), so a tip that lands over its item can't steal the
// hover that keeps it up and flicker.
PopupWindow {
    id: tip

    property string text: ""
    property bool shown: false
    property bool below: false
    property Item target: parent
    property int delay: 500
    // Longer text wraps at this width.
    property int maxWidth: 360

    anchor.item: target
    anchor.rect.x: target ? target.width / 2 : 0
    anchor.rect.y: below ? (target ? target.height + 6 : 0) : -6
    anchor.edges: below ? Edges.Bottom : Edges.Top
    anchor.gravity: below ? Edges.Bottom : Edges.Top
    grabFocus: false
    color: "transparent"
    implicitWidth: label.width + 16
    implicitHeight: label.implicitHeight + 10
    visible: false
    mask: Region {}

    function update() {
        if (shown && text !== "") {
            if (!visible && !wait.running)
                wait.restart();
        } else {
            wait.stop();
            visible = false;
        }
    }
    onShownChanged: update()
    onTextChanged: update()

    Timer {
        id: wait
        interval: tip.delay
        onTriggered: tip.visible = tip.shown && tip.text !== "" && tip.target !== null && tip.target.visible
    }

    Rectangle {
        anchors.fill: parent
        radius: Style.radiusSmall
        color: Style.popover
        border.width: 1
        border.color: Style.alpha(Style.border, 0.6)

        // The text's natural width, for wrapping only what's longer than maxWidth.
        Label {
            id: natural
            visible: false
            text: tip.text
            role: "secondary"
        }
        Label {
            id: label
            anchors.centerIn: parent
            width: Math.min(tip.maxWidth, Math.ceil(natural.implicitWidth))
            text: tip.text
            role: "secondary"
            wrapMode: Text.Wrap
        }
    }
}
