import QtQuick
import "nyx/Ui" as Nyx
Rectangle {
 property alias text: label.text
 property bool error: false
 implicitHeight: label.implicitHeight+18
 color: Nyx.Style.alpha(error?Nyx.Style.danger:Nyx.Style.warning,.09)
 radius:Nyx.Style.radiusSmall
 Plain { id:label;x:9;y:9;width:parent.width-18;color:parent.error?Nyx.Style.danger:Nyx.Style.warning;role:"caption" }
}
