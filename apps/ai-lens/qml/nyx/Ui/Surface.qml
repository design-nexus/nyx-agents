import QtQuick
import QtQuick.Effects

// The body of a panel, popover or dialog: `@theme_popover` (or `kind: "card"` /
// "panel"), `radius_dialog`, a hairline when the theme isn't blended, and a soft
// shadow. Leave `shadowRoom` pixels around it in the window for the shadow.
Item {
    id: s

    property string kind: "popover"
    property real radius: kind === "card" ? Style.radiusCard : Style.radiusDialog
    property color color: ({ popover: Style.popover, card: Style.card, panel: Style.panel, surface: Style.surface })[kind] || Style.popover
    property bool shadow: true
    property bool outline: !Style.blended || kind === "popover"
    readonly property int shadowRoom: 18
    default property alias content: body.data
    property alias body: body

    MultiEffect {
        anchors.fill: bg
        source: bg
        visible: s.shadow
        shadowEnabled: true
        shadowColor: Style.alpha(Style.shadow, 0.55 * Style.shadowIntensity / 0.5)
        shadowBlur: 0.8
        shadowVerticalOffset: 4
        shadowHorizontalOffset: 0
        blurMax: 24
        paddingRect: Qt.rect(0, 0, 0, 0)
    }

    Rectangle {
        id: bg
        anchors.fill: parent
        radius: s.radius
        color: s.color
        border.width: s.outline ? 1 : 0
        border.color: Style.alpha(Style.border, Style.light ? 0.9 : 0.55)
    }

    Item {
        id: body
        anchors.fill: parent
    }
}
