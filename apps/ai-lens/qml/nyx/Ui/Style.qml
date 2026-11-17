pragma Singleton

// Nyx Design 2.0 for shell surfaces in QML: the system theme's colours under the
// names the GTK shell components use (`@theme_card`, `row_hover`, `field`, …), and the
// sizes from STYLE.md. Built on NyxTheme, so everything follows a theme change at once.
//
//   import Nyx.Ui
//   Rectangle { color: Style.card; radius: Style.radiusCard }

import QtQuick
import Quickshell
import "../Theme"

Singleton {
    id: s

    readonly property var l: NyxTheme.legacy
    readonly property var fx: NyxTheme.effects

    function c(name, fallback) {
        return NyxTheme.qmlColor(l[name] || fallback);
    }
    // GTK's mix(a, b, t).
    // Colours may come as names or "#rrggbb" strings (theme swatches, plugin colours).
    function toColor(c) {
        return typeof c === "string" ? Qt.lighter(c, 1.0) : c;
    }
    function mix(a, b, t) {
        a = toColor(a);
        b = toColor(b);
        return Qt.rgba(a.r + (b.r - a.r) * t, a.g + (b.g - a.g) * t, a.b + (b.b - a.b) * t, a.a + (b.a - a.a) * t);
    }
    function alpha(col, a) {
        col = toColor(col);
        return Qt.rgba(col.r, col.g, col.b, col.a * a);
    }

    readonly property bool light: NyxTheme.light
    readonly property bool blended: fx.blended !== false

    readonly property color bg: c("bg", "#1e1e2e")
    readonly property color surface: c("surface", "#181825")
    readonly property color panel: c("panel", "#181825")
    readonly property color card: c("card", "#313244")
    readonly property color popover: c("popover", "#181825")
    readonly property color raised: c("raised", "#45475a")
    readonly property color text: c("text", "#cdd6f4")
    readonly property color dimText: c("dim_text", "#a6adc8")
    readonly property color tertiary: c("text_tertiary", "#7f849c")
    readonly property color accent: c("accent", "#89b4fa")
    readonly property color onAccent: c("on_accent", "#11111b")
    readonly property color border: c("border", "#45475a")
    readonly property color divider: c("divider", "#313244")
    readonly property color danger: c("danger", "#f38ba8")
    readonly property color warning: c("warning", "#f9e2af")
    readonly property color success: c("success", "#a6e3a1")
    readonly property color shadow: c("shadow", "#11111b")
    readonly property color muted: c("muted", "#585b70")

    readonly property real hoverAmount: fx.hover !== undefined ? fx.hover : 0.07
    readonly property real selectionAmount: fx.selection !== undefined ? fx.selection : 0.18
    readonly property color rowHover: mix(bg, accent, hoverAmount)
    readonly property color selected: mix(bg, accent, selectionAmount)
    readonly property color liftActive: mix(card, accent, 0.1)
    readonly property color field: alpha(text, 0.035)
    readonly property color track: alpha(text, 0.1)
    readonly property color hairline: blended ? "transparent" : mix(bg, border, 0.82)
    readonly property color veil: alpha(mix(bg, shadow, 0.45), 0.82)
    readonly property color focus: NyxTheme.colors.border ? NyxTheme.colors.border.focus : accent
    readonly property real shadowIntensity: fx.shadow_intensity !== undefined ? fx.shadow_intensity : 0.5

    // A plugin view's colour: a theme token name (nyx-bar's docs/plugins.md) or
    // `#rrggbb[aa]`. Anything else is no colour.
    function token(name) {
        if (!name)
            return "transparent";
        if (name[0] === "#")
            return /^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$/.test(name) ? NyxTheme.qmlColor(name) : "transparent";
        const t = {
            bg: bg, surface: surface, text: text, dim_text: dimText, accent: accent, on_accent: onAccent,
            border: border, muted: muted, highlight: c("highlight", "#313244"), danger: danger, panel: panel,
            card: card, raised: raised, popover: popover, text_tertiary: tertiary, divider: divider,
            warning: warning, success: success, hover: rowHover, field: field, track: track, hairline: hairline,
            shadow: shadow, nav_active: selected, row_hover: rowHover, lift: card, lift_active: liftActive, veil: veil,
            focus: focus
        }[name];
        return t !== undefined ? t : "transparent";
    }

    // Shape and density (STYLE.md §4, Balanced).
    readonly property int radiusSmall: NyxTheme.radius.small
    readonly property int radiusCard: NyxTheme.radius.card
    readonly property int radiusDialog: NyxTheme.radius.dialog
    readonly property int controlHeight: NyxTheme.geometry.control_height || 32
    readonly property int rowHeight: 36
    readonly property int iconButton: 32
    readonly property int padding: 14
    readonly property int gap: 12
    readonly property int spacing: 8

    // Type (STYLE.md §3).
    readonly property string font: NyxTheme.typography.body.family
    readonly property string mono: NyxTheme.typography.monospace ? NyxTheme.typography.monospace.family : "monospace"
    readonly property int body: 13
    readonly property real secondary: 12
    readonly property int caption: 11
    readonly property real title: 13.5
    readonly property int readout: 22

    // Motion: 0 under Reduce motion.
    readonly property bool reducedMotion: NyxTheme.motion.reduced === true
    readonly property int fast: NyxTheme.motion.fast
    readonly property int normal: NyxTheme.motion.normal
    readonly property int slow: NyxTheme.motion.slow
    readonly property int panelDuration: reducedMotion ? 0 : 200
    readonly property var easing: NyxTheme.motion.standard
    readonly property var enter: NyxTheme.motion.enter
    readonly property var exit: NyxTheme.motion.exit
}
