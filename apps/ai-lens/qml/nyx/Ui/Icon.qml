import QtQuick
import QtQuick.Effects
import Quickshell
import "../Theme"

// An icon by icon-theme name or file path. Symbolic icons (names ending in
// `-symbolic`, or `recolor: true`) are drawn in `color`, as GTK draws them in
// currentColor.
Item {
    id: icon

    property string name: ""
    property string fallback: "application-x-executable"
    property int size: 16
    property color color: Style.dimText
    property bool recolor: name.endsWith("-symbolic")
    readonly property bool ok: img.status === Image.Ready

    // The suite's own symbolics, shipped here (nyx-ui's chrome draws the same ones in
    // GTK). The theme's "emblem-system" is Settings too: the suite's two sliders, as
    // STYLE.md asks, not whatever the icon theme draws (a sun in some).
    readonly property var own: ({ "nyx-settings-symbolic": "icons/nyx-settings-symbolic.svg", "emblem-system-symbolic": "icons/nyx-settings-symbolic.svg" })

    readonly property string source: {
        if (name === "")
            return "";
        if (own[name])
            return Qt.resolvedUrl(own[name]);
        if (name.startsWith("/"))
            return "file://" + name;
        if (name.startsWith("file:") || name.startsWith("data:") || name.startsWith("image:") || name.startsWith("qrc:"))
            return name;
        return Quickshell.iconPath(name, fallback);
    }

    implicitWidth: size
    implicitHeight: size

    Image {
        id: img
        anchors.fill: parent
        source: icon.source
        sourceSize: Qt.size(icon.size * 2, icon.size * 2)
        fillMode: Image.PreserveAspectFit
        smooth: true
        mipmap: true
        asynchronous: true
        // Once the icon theme has been redrawn the cached pictures are stale.
        cache: NyxTheme.iconGeneration === 0
        visible: !icon.recolor
    }
    Connections {
        target: NyxTheme
        function onIconGenerationChanged() {
            if (!icon.source.startsWith("image://icon/"))
                return;
            img.source = "";
            img.source = Qt.binding(() => icon.source);
        }
    }

    MultiEffect {
        anchors.fill: parent
        source: img
        visible: icon.recolor
        colorization: 1.0
        colorizationColor: icon.color
        brightness: 1.0
    }
}
