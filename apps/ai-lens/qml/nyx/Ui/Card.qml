import QtQuick

// A card: `@theme_card`, `radius_card`, a hairline only when the theme isn't
// blended, 14 px inside.
Rectangle {
    property int padding: Style.padding
    default property alias content: inner.data
    property alias inner: inner

    radius: Style.radiusCard
    color: Style.card
    border.width: Style.blended ? 0 : 1
    border.color: Style.alpha(Style.border, 0.6)

    Item {
        id: inner
        anchors.fill: parent
        anchors.margins: parent.padding
    }
}
