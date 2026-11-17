import QtQuick

// A divider line: `@theme_divider`, or spacing only when the theme is blended and
// `always` is false.
Rectangle {
    property bool vertical: false
    property bool always: true

    implicitWidth: vertical ? 1 : 100
    implicitHeight: vertical ? 100 : 1
    color: always || !Style.blended ? Style.alpha(Style.divider, Style.light ? 1 : 0.8) : "transparent"
}
