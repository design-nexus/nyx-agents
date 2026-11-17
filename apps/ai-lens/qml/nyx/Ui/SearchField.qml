import QtQuick

// A search field: `field` fill, a search icon, a placeholder and an optional key hint;
// focus shows an accent outline. Escape clears it first, then is left to the panel.
Rectangle {
    id: f

    property alias text: input.text
    property string placeholder: "Search"
    property string hint: ""
    property string icon: "edit-find-symbolic"
    property alias input: input
    property bool password: false

    signal accepted
    signal escaped
    signal keyDown
    signal keyUp

    function clear() {
        input.text = "";
    }
    function focusInput() {
        input.forceActiveFocus();
    }

    implicitHeight: Style.controlHeight + 4
    implicitWidth: 260
    radius: Style.radiusSmall
    color: Style.field
    border.width: input.activeFocus ? 2 : 1
    border.color: input.activeFocus ? Style.accent : Style.alpha(Style.border, 0.5)

    Icon {
        id: ic
        visible: f.icon !== ""
        anchors.left: parent.left
        anchors.leftMargin: 10
        anchors.verticalCenter: parent.verticalCenter
        name: f.icon
        size: 16
        recolor: true
        color: Style.tertiary
    }

    TextInput {
        id: input
        anchors.left: ic.visible ? ic.right : parent.left
        anchors.leftMargin: 8
        anchors.right: hintLabel.visible ? hintLabel.left : parent.right
        anchors.rightMargin: 10
        anchors.verticalCenter: parent.verticalCenter
        color: Style.text
        selectionColor: Style.alpha(Style.accent, 0.4)
        selectedTextColor: Style.text
        font.family: Style.font
        font.pixelSize: Style.body
        clip: true
        echoMode: f.password ? TextInput.Password : TextInput.Normal
        Accessible.role: Accessible.EditableText
        Accessible.name: f.placeholder
        Accessible.passwordEdit: f.password
        onAccepted: f.accepted()
        Keys.onEscapePressed: e => {
            if (input.text !== "") {
                input.text = "";
                e.accepted = true;
            } else {
                f.escaped();
                e.accepted = false;
            }
        }
        Keys.onDownPressed: f.keyDown()
        Keys.onUpPressed: f.keyUp()

        Label {
            anchors.fill: parent
            visible: input.text === "" && !input.preeditText
            text: f.placeholder
            color: Style.tertiary
        }
    }

    Label {
        id: hintLabel
        visible: f.hint !== "" && input.text === ""
        anchors.right: parent.right
        anchors.rightMargin: 10
        anchors.verticalCenter: parent.verticalCenter
        text: f.hint
        role: "caption"
        color: Style.tertiary
    }
}
