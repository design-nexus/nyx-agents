import QtQuick
import Quickshell.Io
QtObject {
 id:root
 property bool enabled:false
 property string binary: ""
 property int sequence: 0
 property var pending: ({})
 property bool ready: false
 signal reply(string operation,var result,string error,var request)
 property Process process: Process {
  command:[root.binary,"--stdio"]
  running:root.enabled
  stdinEnabled:true
  stdout:SplitParser { onRead: data=>{try{let v=JSON.parse(data);let p=root.pending[v.id];if(!p)return;delete root.pending[v.id];root.reply(p.op,v.result||{},v.error||"",p)}catch(e){root.reply("transport",{},"Invalid backend response",{})}} }
  onStarted:{root.ready=true;root.reply("ready",{},"",{})}
  onExited:{root.ready=false;root.pending={};if(root.enabled)root.reply("transport",{},"Backend disconnected. Your editor text is retained.",{})}
 }
 function send(op,args){if(!ready){reply(op,{},"Backend unavailable",args||{});return -1}let id=++sequence;let r=Object.assign({},args||{},{op:op,id:id});pending[id]=r;process.write(JSON.stringify(r)+"\n");return id}
 function restart(){process.running=Qt.binding(function(){return root.enabled})}
}
