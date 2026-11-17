import QtQuick
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
Scope {
 id:root
 property bool opened:false
 IpcHandler {target:"nyx-ai-lens";function toggle():void{root.opened=!root.opened}
 function show():void{root.opened=true}
 function hide():void{content.closeRequested()}}
 PanelWindow {
  visible:root.opened
  anchors.top:true;anchors.right:true
  margins.top:48;margins.right:18
  implicitWidth:Math.min(440,screen?screen.width-24:440)
  implicitHeight:Math.min(740,screen?screen.height-80:740)
  color:"transparent"
  exclusiveZone:0
  WlrLayershell.layer:WlrLayer.Overlay
  WlrLayershell.keyboardFocus:WlrKeyboardFocus.OnDemand
  LensPanel {id:content;anchors.fill:parent;active:root.opened;focus:true;onCloseRequested:root.opened=false}
 }
}
