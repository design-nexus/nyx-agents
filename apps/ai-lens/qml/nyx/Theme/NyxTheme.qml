pragma Singleton

// The Nyx theme for Quickshell components: the system theme's resolved Nyx
// Design 2.0 tokens as reactive properties. Every shell component and plugin uses
// this one object; none reads theme files itself. Shell components always show the
// system theme.
//
//   import "../Theme"
//   Rectangle {
//       color: NyxTheme.colors.surface.primary
//       radius: NyxTheme.radius.medium
//       Text { color: NyxTheme.colors.text.primary; font.pixelSize: NyxTheme.typography.body.size }
//   }
//
// The theme engine writes one immutable snapshot per change
// ($XDG_RUNTIME_DIR/nyx/theme/system.json, replaced atomically), so a change is
// applied whole: every property here moves to the new generation at once. Until a
// snapshot is readable the built-in default theme (fallback.js) is used, so
// components always have every token.

import QtQuick
import Quickshell
import Quickshell.Io
import "fallback.js" as Fallback

Singleton {
    id: root

    /// Where the engine keeps the snapshot (as nyx-theme-core's `paths::runtime_dir`).
    readonly property string runtimeDir: Quickshell.env("XDG_RUNTIME_DIR")
        ? Quickshell.env("XDG_RUNTIME_DIR") + "/nyx/theme"
        : (Quickshell.env("XDG_CACHE_HOME") || Quickshell.env("HOME") + "/.cache") + "/nyx/theme-runtime"
    readonly property string snapshotPath: runtimeDir + "/system.json"

    /// The whole snapshot in use (format 1).
    property var snapshot: Fallback.snapshot
    /// Whether a real snapshot has been read (false while on the built-in fallback).
    property bool live: false
    /// The theme engine's generation on screen.
    readonly property int generation: snapshot.generation
    readonly property var theme: snapshot.theme
    readonly property bool light: snapshot.theme.appearance === "light"

    /// `colors.surface.primary`, `colors.text.onAccent`, … (every colour token).
    readonly property var colors: nest(snapshot.tokens.colors)
    /// The established names (`legacy.bg`, `legacy.dim_text`, …) for older components.
    readonly property var legacy: snapshot.legacy
    readonly property var swatches: snapshot.swatches
    readonly property var accents: snapshot.accents

    /// Corner radii in logical px: `small`, `medium` (cards), `large` (dialogs).
    readonly property var radius: ({
        small: snapshot.tokens.geometry.radius_small,
        medium: snapshot.tokens.geometry.radius_card,
        large: snapshot.tokens.geometry.radius_dialog,
        card: snapshot.tokens.geometry.radius_card,
        dialog: snapshot.tokens.geometry.radius_dialog
    })
    /// Spacing in logical px: `xxs` … `xxl`, and `unit`.
    readonly property var spacing: Object.assign({ unit: snapshot.tokens.geometry.spacing_unit }, snapshot.tokens.geometry.spacing)
    readonly property var geometry: snapshot.tokens.geometry

    /// The interface font size the roles scale from, in px. Logical pixels, so
    /// monitor scale factors apply on their own.
    property real basePixelSize: 13
    /// `typography.body.size` (px), `.weight`, `.lineHeight`, `.letterSpacing` (px), `.family`.
    readonly property var typography: roles(snapshot.tokens.typography, basePixelSize)

    /// `motion.fast/normal/slow` (ms, 0 under reduced motion), `motion.standard/enter/exit`
    /// (an easing for `Behavior`/`NumberAnimation`: `type` and `bezierCurve`), `motion.reduced`.
    readonly property var motion: ({
        fast: snapshot.tokens.motion.fast,
        normal: snapshot.tokens.motion.normal,
        slow: snapshot.tokens.motion.slow,
        reduced: snapshot.tokens.motion.reduced,
        standard: easing(snapshot.tokens.motion.easing_standard),
        enter: easing(snapshot.tokens.motion.easing_enter),
        exit: easing(snapshot.tokens.motion.easing_exit)
    })
    readonly property var effects: snapshot.tokens.effects

    /// The system logo in the theme's colours: `Image { source: NyxTheme.logo.source }`.
    /// `recolorable` is false for a logo the theme doesn't colour (a PNG).
    readonly property var logo: ({
        source: logoSource(snapshot.logo),
        recolorable: snapshot.logo ? snapshot.logo.recolorable : true
    })

    function logoSource(l) {
        if (!l)
            return ""
        if (l.path)
            return "file://" + l.path + "?" + l.stamp
        if (l.svg)
            return "data:image/svg+xml;utf8," + encodeURIComponent(l.svg)
        return ""
    }

    /// After a new generation is applied.
    signal changed(int generation)

    /// Goes up when the icon theme in use is redrawn under the same names (Umbra in a new
    /// theme's colours, by nyx-themed or Omarchy's hook), so icons fetch their pictures
    /// again: QML's image cache would keep the old ones.
    property int iconGeneration: 0
    readonly property string iconTheme: Quickshell.env("QS_ICON_THEME") || "Umbra"
    /// Umbra writes its palette here with every build (the whole folder is swapped in).
    readonly property string iconStampPath: (Quickshell.env("XDG_DATA_HOME") || Quickshell.env("HOME") + "/.local/share") + "/icons/" + iconTheme + "/umbra.json"
    property string _iconStamp: ""

    onChanged: { iconWatch.tries = 0; iconWatch.restart() }

    FileView {
        id: iconStamp
        path: root.iconStampPath
        printErrors: false
        onLoaded: {
            const t = text()
            if (root._iconStamp !== "" && t !== root._iconStamp)
                root.iconGeneration++
            root._iconStamp = t
        }
    }

    // A theme change starts the redraw; look for it for half a minute after.
    Timer {
        id: iconWatch
        property int tries: 0
        interval: 1500
        repeat: true
        onTriggered: {
            iconStamp.reload()
            if (++tries >= 20)
                stop()
        }
    }

    Component.onCompleted: iconStamp.reload()

    function nest(flat) {
        var out = {}
        for (var key in flat) {
            var parts = key.split(".")
            var group = out[parts[0]] || (out[parts[0]] = {})
            group[parts[1]] = qmlColor(flat[key])
        }
        return out
    }

    /// `#rrggbbaa` (CSS) as `#aarrggbb` (QML).
    function qmlColor(c) {
        if (typeof c === "string" && c.length === 9 && c[0] === "#")
            return "#" + c.substr(7, 2) + c.substr(1, 6)
        return c
    }

    function firstFamily(stack, fallback) {
        if (!stack)
            return fallback
        return stack.split(",")[0].trim().replace(/"/g, "")
    }

    function roles(t, base) {
        var out = {}
        for (var name in t.roles) {
            var r = t.roles[name]
            var size = Math.round(base * r.size)
            out[name] = {
                size: size,
                weight: r.weight,
                lineHeight: r.line_height,
                letterSpacing: r.letter_spacing * size,
                family: r.family === "mono" ? firstFamily(t.mono, "monospace") : firstFamily(t.ui, Qt.application.font.family)
            }
        }
        return out
    }

    function easing(css) {
        var s = (css || "ease-out").trim()
        var m = s.match(/^cubic-bezier\(([^)]*)\)$/)
        if (m) {
            var n = m[1].split(",").map(function (x) { return parseFloat(x) })
            if (n.length === 4 && n.every(function (x) { return !isNaN(x) }))
                return { type: Easing.BezierSpline, bezierCurve: [n[0], n[1], n[2], n[3], 1, 1] }
        }
        var named = { "linear": Easing.Linear, "ease": Easing.InOutQuad, "ease-in": Easing.InCubic, "ease-out": Easing.OutCubic, "ease-in-out": Easing.InOutCubic }
        return { type: named[s] !== undefined ? named[s] : Easing.OutCubic, bezierCurve: [] }
    }

    /// Take a snapshot if it is complete and newer than the one on screen.
    function accept(text) {
        var s
        try {
            s = JSON.parse(text)
        } catch (e) {
            return
        }
        if (!s || s.format !== 1 || !s.tokens || !s.tokens.colors || !s.theme)
            return
        // A restarted session starts counting again; anything else older is stale.
        if (live && s.generation < snapshot.generation && s.generation > 1)
            return
        if (live && s.generation === snapshot.generation && s.theme.id === snapshot.theme.id)
            return
        snapshot = s
        live = true
        changed(s.generation)
    }

    FileView {
        id: file
        path: root.snapshotPath
        watchChanges: true
        printErrors: false
        onFileChanged: reload()
        onLoaded: root.accept(text())
        // No snapshot yet (the theme service hasn't run): have the engine write one.
        onLoadFailed: if (!writer.running && !writer.tried) writer.running = true
    }

    Process {
        id: writer
        property bool tried: false
        command: ["nyx-theme", "snapshot"]
        onRunningChanged: if (!running) { tried = true; file.reload() }
    }

    // A file that doesn't exist yet can't be watched: until the first snapshot is
    // read (the theme service may start after the shell), look for it now and then.
    Timer {
        interval: 2000
        repeat: true
        running: !root.live
        onTriggered: file.reload()
    }
}
