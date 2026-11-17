import QtQuick
import QtQuick.Controls as Controls
import QtQuick.Layouts
import "nyx/Ui" as Nyx

Controls.ComboBox {
    id: control
    property var agents: []
    property string selectedAgentId: ""
    readonly property var options: [
        {
            id: "",
            name: "All agents"
        }
    ].concat(agents.map(a => ({
                id: a.id,
                name: a.name
            })))
    signal agentSelected(string id)
    model: control.options
    textRole: "name"
    valueRole: "id"
    currentIndex: Math.max(0, control.options.findIndex(a => a.id === selectedAgentId))
    onActivated: index => agentSelected(control.options[index].id)
    implicitHeight: Nyx.Style.controlHeight
    implicitWidth: 200
    leftPadding: 10
    rightPadding: 32
    topPadding: 0
    bottomPadding: 0
    font.family: Nyx.Style.font
    font.pixelSize: Nyx.Style.body
    Accessible.name: "Filter sessions by agent"

    contentItem: Plain {
        text: control.displayText
        verticalAlignment: Text.AlignVCenter
        wrapMode: Text.NoWrap
        maximumLineCount: 1
    }
    indicator: Nyx.Icon {
        name: "pan-down-symbolic"
        size: 16
        x: control.width - width - 10
        y: (control.height - height) / 2
        color: Nyx.Style.dimText
    }
    background: Rectangle {
        color: control.hovered ? Nyx.Style.rowHover : Nyx.Style.field
        radius: Nyx.Style.radiusSmall
        border.width: control.visualFocus ? 2 : 1
        border.color: control.visualFocus ? Nyx.Style.focus : Nyx.Style.border
    }
    delegate: Controls.ItemDelegate {
        required property var modelData
        required property int index
        objectName: "session-filter-option-" + index
        width: ListView.view ? ListView.view.width : control.width - 12
        height: 32
        padding: 8
        hoverEnabled: true
        highlighted: control.highlightedIndex === index
        contentItem: RowLayout {
            spacing: 8
            Plain {
                Layout.fillWidth: true
                text: modelData.name
                wrapMode: Text.NoWrap
                maximumLineCount: 1
                strong: control.currentIndex === index
            }
            Plain {
                Layout.preferredWidth: 16
                text: control.currentIndex === index ? "✓" : ""
                color: Nyx.Style.accent
                horizontalAlignment: Text.AlignHCenter
            }
        }
        background: Rectangle {
            radius: Nyx.Style.radiusSmall
            color: parent.highlighted || parent.hovered ? Nyx.Style.rowHover : control.currentIndex === parent.index ? Nyx.Style.selected : "transparent"
        }
    }
    popup: Controls.Popup {
        y: control.height + 4
        width: control.width
        height: Math.min(optionList.contentHeight + padding * 2, Controls.Overlay.overlay ? Controls.Overlay.overlay.height - 16 : 240)
        padding: 6
        margins: 8
        popupType: Controls.Popup.Item
        contentItem: ListView {
            id: optionList
            clip: true
            implicitHeight: contentHeight
            model: control.delegateModel
            currentIndex: control.highlightedIndex
            highlightMoveDuration: 0
            boundsBehavior: Flickable.StopAtBounds
            Controls.ScrollIndicator.vertical: Controls.ScrollIndicator {}
        }
        background: Rectangle {
            color: Nyx.Style.popover
            radius: Nyx.Style.radiusCard
            border.width: 1
            border.color: Nyx.Style.border
        }
    }
}
