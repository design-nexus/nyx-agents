import QtQuick
import qs.Commons
import qs.Ui
Panel {
 id:root
 moduleName:"design-nexus.ai-lens"
 ipcTarget:"design-nexus.ai-lens"
 implicitWidth:button.implicitWidth
 implicitHeight:button.implicitHeight
 BarIconButton {id:button;anchors.fill:parent;bar:root.bar;text:"✧";tooltipText:"AI Lens";active:root.opened;onPressed:root.toggle()}
 KeyboardPanel {
  id:popup
  padding:0
  bar:root.bar;owner:root;anchorItem:button;open:root.opened
  contentWidth:popup.fittedContentWidth(440)
  contentHeight:popup.fittedContentHeight(740,740)
  focusTarget:content
  LensPanel {id:content;anchors.fill:parent;active:root.opened;focus:true;onCloseRequested:root.close()}
 }
}
