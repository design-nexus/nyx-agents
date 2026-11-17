import QtQuick

// A count or short label in a small pill: accent by default, `tone` "warning",
// "danger" or "neutral".
Rectangle {
    id: b

    property string text: ""
    property string tone: "accent"
    readonly property color toneColor: ({ accent: Style.accent, warning: Style.warning, danger: Style.danger, success: Style.success, neutral: Style.muted })[tone] || Style.accent

    implicitHeight: 16
    implicitWidth: Math.max(implicitHeight, label.implicitWidth + 10)
    radius: height / 2
    color: toneColor
    visible: text !== ""

    Label {
        id: label
        anchors.centerIn: parent
        text: b.text
        size: 10
        font.weight: Font.DemiBold
        color: b.tone === "neutral" ? Style.text : Style.onAccent
    }
}
