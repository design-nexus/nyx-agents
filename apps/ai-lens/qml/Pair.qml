import QtQuick
import "nyx/Ui" as Nyx
Item {
 property string label: ""
 property string value: ""
 implicitHeight: Math.max(left.implicitHeight,right.implicitHeight)+4
 Plain { id:left;width:parent.width*.43;text:parent.label;dim:true;role:"caption" }
 Plain { id:right;x:parent.width*.45;width:parent.width*.55;text:parent.value;horizontalAlignment:Text.AlignRight;role:"caption" }
}
