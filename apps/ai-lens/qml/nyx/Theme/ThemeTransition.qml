// How every Nyx shell component moves to a new theme: colours cross-fade over the
// theme's normal duration with its standard easing, or switch at once under reduced
// motion. A change that arrives mid-animation starts from where the colour is, so
// nothing is left half-way.
//
//   Rectangle {
//       color: NyxTheme.colors.surface.primary
//       Behavior on color { ThemeTransition {} }
//   }

import QtQuick

ColorAnimation {
    duration: NyxTheme.motion.reduced ? 0 : NyxTheme.motion.normal
    easing.type: NyxTheme.motion.standard.type
    easing.bezierCurve: NyxTheme.motion.standard.bezierCurve
}
