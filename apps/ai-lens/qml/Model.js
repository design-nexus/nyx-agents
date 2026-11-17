.pragma library
function array(x){return x&&typeof x.length==='number'?Array.prototype.slice.call(x):[]}
function status(s){if(s.lifecycle==='lost')return 'Disconnected';if(s.lifecycle==='exited')return 'Ended';if(s.status&&s.status.activity==='blocked')return 'Waiting for input';if(s.status&&s.status.activity==='error')return 'Error';if(s.status&&s.status.activity==='working')return 'Active';if(s.status&&s.status.activity==='idle')return 'Idle';if(s.lifecycle==='starting')return 'Starting';return 'Unknown'}
function need(s){return s.status==='Waiting for input'||s.status==='Error'}
function live(s){return s.live===true}
function sessions(m){m=unified(m);let out=array(m.sessions).map(s=>({id:s.id,agent:s.harness,source:s._source_name||'',title:s.title||s.name||'Untitled session',cwd:s.cwd||'',branch:s.worktree?s.worktree.branch||'':'',status:status(s),live:s.lifecycle==='running'||s.lifecycle==='starting',message:s.status?s.status.message||'':'',updated:s.status?s.status.since:s.created,model:'',raw:s}));for(let u of array(m.usage)){for(let s of array(u.sessions)){if(out.some(x=>x.agent===u.harness&&x.raw.harness_session===s.id))continue;out.push({id:'usage:'+u.harness+':'+s.id,agent:u.harness,source:s._source_name||'',title:s.title||'Untitled session',cwd:s.cwd||'',branch:'',status:s.active?'Running':'Idle',live:!!s.active,message:'',updated:Number(s.updated||0)*1000,model:s.model||'',prompts:s.prompts,tokens:s.tokens,raw:s})}}return out.sort((a,b)=>Number(b.live)-Number(a.live)||b.updated-a.updated)}
function catalog(m){m=unified(m);let ids={};array(m.harnesses).filter(h=>h.path).forEach(h=>ids[h.id]={id:h.id,name:h.name,version:h.version});array(m.usage).forEach(u=>{ids[u.harness]=Object.assign(ids[u.harness]||{id:u.harness,name:u.name},{usage:u})});let ss=sessions(m);ss.forEach(s=>{if(!ids[s.agent])ids[s.agent]={id:s.agent,name:s.agent}});return Object.values(ids).map(a=>{a.sessions=ss.filter(s=>s.agent===a.id);a.current=a.sessions.find(need)||a.sessions.find(s=>s.live)||a.sessions[0];a.quota=displayQuota(a.id,(a.usage||{}).quota||{});a.attention=a.sessions.some(need);a.working=a.sessions.some(s=>s.status==='Active');a.state=a.attention?a.sessions.find(need).status:a.working?'Active':a.current?a.current.status:(a.usage||{}).working?'Running':'Unknown';return a}).sort((a,b)=>Number(b.attention)-Number(a.attention)||Number(b.working)-Number(a.working)||a.name.localeCompare(b.name))}
function shortPath(p){return String(p||'').split('/').filter(x=>x).slice(-2).join('/')||'Workspace unavailable'}
function time(t){let delta=Date.now()-Number(t);if(!t||!isFinite(delta))return 'Not exposed';let sec=Math.max(0,Math.floor(delta/1000));if(sec<60)return 'just now';if(sec<3600)return Math.floor(sec/60)+'m ago';if(sec<86400)return Math.floor(sec/3600)+'h ago';return Math.floor(sec/86400)+'d ago'}
function reset(t){if(!t||!isFinite(Number(t)))return 'Reset unavailable';let diff=Number(t)*1000-Date.now();if(diff<=0)return 'Resets now';let mins=Math.floor(diff/60000),hours=Math.floor(mins/60),days=Math.floor(hours/24);return 'Resets in '+(days>0?days+'d '+hours%24+'h':hours>0?hours+'h '+mins%60+'m':Math.max(1,mins)+'m')}
function compact(n){if(n===undefined||n===null)return '—';n=Number(n);if(n>=1000000)return (n/1000000).toFixed(1).replace(/\.0$/,'')+'M';if(n>=1000)return (n/1000).toFixed(n>=100000?0:1).replace(/\.0$/,'')+'k';return String(n)}
function remaining(w){if(typeof w.used_percent!=="number")return -1;let x=Number(w.used_percent);return isFinite(x)?Math.max(0,Math.min(100,100-x)):-1}
function number(n){if(n===undefined||n===null)return 'Unavailable';return Number(n).toLocaleString(Qt.locale(),'f',0)}
function gib(n){return n===undefined||n===null?'Unavailable':(Number(n)/1073741824).toFixed(1)+' GiB'}


function defaultEnabled(a){return !!(a.usage&&a.usage.has_records)||array(a.quota.windows).length>0}
function enabled(a,choices){
 if(choices&&Object.prototype.hasOwnProperty.call(choices,a.id)&&typeof choices[a.id]==='boolean')return choices[a.id];
 if(a.id==='xai'&&a.usage&&a.usage.member_defaults){let defaults=a.usage.member_defaults;return Object.keys(defaults).some(id=>choices&&Object.prototype.hasOwnProperty.call(choices,id)&&typeof choices[id]==='boolean'?choices[id]:defaults[id])}
 return defaultEnabled(a);
}
function agents(m,choices){return catalog(m).filter(a=>enabled(a,choices))}
function parseChoices(text){try{let v=JSON.parse(String(text||'{}'));return v&&typeof v==='object'&&!Array.isArray(v)?v:{}}catch(e){return {}}}

// One display identity and one provider-reported weekly pool. Per-product shares
// and legacy API/on-demand pools must never become additive subscription quotas.
function xaiMember(id){return id==='grok'||id==='cursor'}
function clientName(id){return id==='grok'?'Grok Build':'Cursor CLI'}
function sums(objects){
 let out={};for(let object of objects){for(let key of Object.keys(object||{})){let v=object[key];if(typeof v==='number'&&isFinite(v))out[key]=(out[key]||0)+v}}return out;
}
function grouped(rows,key){
 let groups={};for(let row of rows){let id=String(row[key]||'');if(!groups[id])groups[id]=[];groups[id].push(row)}
 return Object.keys(groups).map(id=>{let value=sums(groups[id]);value[key]=id;return value});
}
function sharedQuota(usages){
 let candidates=[];
 for(let u of usages){let q=u.quota||{};let windows=array(q.windows).filter(w=>u.harness==='grok'?w.id==='grok-weekly':/^(week|weekly)$/i.test(String(w.label||''))||/weekly/i.test(String(w.id||'')));
  for(let w of windows){if(typeof w.used_percent==='number'&&isFinite(w.used_percent))candidates.push({q:q,w:w,source:u.harness})}
 }
 // Prefer xAI's overall pool over product breakdowns. A successful alternate
 // reading can replace a failed source; never add/average their percentages.
 candidates.sort((a,b)=>Number(!!a.q.error)-Number(!!b.q.error)||Number(b.source==='grok')-Number(a.source==='grok')||Number(b.q.updated||0)-Number(a.q.updated||0));
 let chosen=candidates[0],primary=usages.find(u=>u.harness==='grok'),secondary=usages.find(u=>u.harness==='cursor');
 let plan=(primary&&primary.quota||{}).plan||(secondary&&secondary.quota||{}).plan;
 if(!chosen)return {available:usages.some(u=>(u.quota||{}).available),enabled:usages.some(u=>(u.quota||{}).enabled),plan:plan,windows:[],source:'Shared weekly allowance unavailable'};
 let q=Object.assign({},chosen.q);q.plan=plan;q.windows=[Object.assign({},chosen.w,{id:'xai-weekly',label:'Week'})];q.source=clientName(chosen.source)+' · shared weekly allowance';return q;
}
function unified(m){
 if(m._xaiUnified)return m;
 let harnesses=array(m.harnesses),usages=array(m.usage),managed=array(m.sessions);
 if(!harnesses.some(h=>xaiMember(h.id))&&!usages.some(u=>xaiMember(u.harness))&&!managed.some(s=>xaiMember(s.harness)))return m;
 let members=usages.filter(u=>xaiMember(u.harness)),installed=harnesses.filter(h=>xaiMember(h.id));
 let usage={harness:'xai',name:'xAI',has_records:members.some(u=>u.has_records),working:members.some(u=>u.working),today:sums(members.map(u=>u.today)),totals:sums(members.map(u=>u.totals)),quota:sharedQuota(members),member_defaults:{}};
 for(let id of ['grok','cursor']){let u=members.find(u=>u.harness===id);if(u||installed.some(h=>h.id===id))usage.member_defaults[id]=!!(u&&(u.has_records||array((u.quota||{}).windows).length))}
 let updated=members.map(u=>u.updated).filter(t=>typeof t==='number'&&t>0);if(updated.length)usage.updated=Math.min(...updated);
 let last=members.map(u=>u.last_used).filter(t=>typeof t==='number');if(last.length)usage.last_used=Math.max(...last);
 usage.days=grouped(members.reduce((a,u)=>a.concat(array(u.days)),[]),'date').sort((a,b)=>a.date.localeCompare(b.date));
 usage.models=grouped(members.reduce((a,u)=>a.concat(array(u.models)),[]),'name');
 usage.tools=grouped(members.reduce((a,u)=>a.concat(array(u.tools)),[]),'name').sort((a,b)=>(b.calls||0)-(a.calls||0));
 usage.sessions=members.reduce((a,u)=>a.concat(array(u.sessions).map(s=>Object.assign({},s,{id:u.harness+':'+s.id,_source_name:clientName(u.harness)}))),[]).sort((a,b)=>(b.updated||0)-(a.updated||0));
 let combined={id:'xai',name:'xAI',path:(installed.find(h=>h.path)||{}).path};
 return Object.assign({},m,{_xaiUnified:true,harnesses:harnesses.filter(h=>!xaiMember(h.id)).concat([combined]),usage:usages.filter(u=>!xaiMember(u.harness)).concat([usage]),sessions:managed.map(s=>xaiMember(s.harness)?Object.assign({},s,{harness:'xai',_source_name:clientName(s.harness),harness_session:s.harness_session?s.harness+':'+s.harness_session:s.harness_session}):s)});
}

function displayQuota(id,quota){
 if(id!=='antigravity')return quota;
 let windows=array(quota.windows),selected=[];
 for(let entry of [{id:'gemini-5h',label:'5 Hours'},{id:'gemini-weekly',label:'Week'},{id:'gemini-argon-wtu-weekly',label:'Argon'}]){
  let window=windows.find(w=>w.id===entry.id);
  if(window)selected.push(Object.assign({},window,{label:entry.label}));
 }
 return Object.assign({},quota,{windows:selected});
}
