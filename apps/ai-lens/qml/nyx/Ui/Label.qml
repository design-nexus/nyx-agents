import QtQuick

// Text in the interface font. Plain text by default: titles, names and notification
// text from apps are untrusted and must not be read as markup.
Text {
    property string role: "body"
    property bool dim: false
    property bool strong: false

    color: dim ? Style.dimText : Style.text
    font.family: Style.font
    // Logical pixels, fractions included (12.5 px row titles), as points.
    property real size: ({ body: Style.body, secondary: Style.secondary, caption: Style.caption, title: Style.title, readout: Style.readout, group: Style.caption })[role] || Style.body
    font.pointSize: size * 0.75
    font.weight: strong || role === "title" || role === "readout" || role === "group" ? Font.DemiBold : Font.Normal
    font.capitalization: role === "group" ? Font.AllUppercase : Font.MixedCase
    font.letterSpacing: role === "group" ? size * 0.06 : 0
    textFormat: Text.PlainText
    elide: Text.ElideRight
    maximumLineCount: wrapMode === Text.NoWrap ? 1 : 1000
    verticalAlignment: Text.AlignVCenter
}
