const fs=require('node:fs');
const vm=require('node:vm');
const path=require('node:path');
const assert=require('node:assert/strict');
const root=path.join(__dirname,'..');
const rust=fs.readFileSync(path.join(root,'src/project_sources.rs'),'utf8');
const web=fs.readFileSync(path.join(root,'src/web.rs'),'utf8');
const code=fs.readFileSync(path.join(root,'assets/project-sources.js'),'utf8');
// Server/client contract: authenticated GET JSON with matching HTML row markup.
assert.match(web,/\.route\("\/projects\/\{id\}\/sources\/status",get\(project_sources::status\)\)/);
assert.match(rust,/pub async fn status\(/);
assert.match(rust,/"rows_html":view\.html/);
assert.match(rust,/"pending":view\.pending/);
assert.match(rust,/StatusCode::ACCEPTED/);
assert.match(rust,/headers\.get\("x-requested-with"\)/);
assert.match(rust,/id='project-sources-rows'/);
assert.match(rust,/id='project-sources-refresh'/);
assert.doesNotMatch(rust,/<a class='buttonlink' href='\/projects\/\{project_id\}#project-sources'>Обновить/);
new vm.Script(code);

function node(){return {value:0,hidden:false,disabled:false,textContent:'',innerHTML:'',dataset:{},
  handlers:{},addEventListener(type,fn){this.handlers[type]=fn;},emit(type,payload){this.handlers[type]?.(payload);}};}
async function testUI(){
  const panel=node(), form=node(), button=node(), uploadNote=node(), progress=node(), picker=node();
  const refresh=node(), refreshNote=node(), rows=node(), count=node();
  picker.files=[{name:'КР.pdf'}];
  form.action='/projects/4/sources/upload';
  form.querySelector=s=>({"button[type=submit]":button,"[role=status]":uploadNote,"progress":progress,"input[type=file]":picker})[s];
  refresh.dataset.statusUrl='/projects/4/sources/status';
  const elements={'project-sources':panel,'project-sources-form':form,'project-sources-refresh':refresh,
    'project-sources-refresh-note':refreshNote,'project-sources-rows':rows,'project-sources-count':count};
  const requests=[];let statusResponse='queued';let assignments=0;let xhrObject;
  const fetch=async (url,options)=>{
    requests.push({url,options});
    return {ok:true,status:200,headers:{get:()=> 'application/json; charset=utf-8'},
      json:async()=>({rows_html:`<tr><td>${statusResponse}</td></tr>`,total:1,
        pending:statusResponse==='queued'?1:0,ready:statusResponse==='ready'?1:0,errors:0})};
  };
  class XHR{
    constructor(){xhrObject=this;this.upload=node();this.handlers={};this.status=202;
      this.responseText=JSON.stringify({queued:1,unchanged:0});}
    open(method,url,async){this.method=method;this.url=url;this.async=async;}
    setRequestHeader(key,value){this.headers??={};this.headers[key]=value;}
    addEventListener(key,fn){this.handlers[key]=fn;}
    send(){this.upload.emit('progress',{lengthComputable:true,loaded:5,total:5});this.handlers.load();}
  }
  const ctx={document:{getElementById:s=>elements[s]},window:{XMLHttpRequest:XHR,FormData:class {},
    location:{pathname:'/projects/4',assign(){assignments++;}}},
    XMLHttpRequest:XHR,FormData:class {},fetch,
    setTimeout:()=>123,clearTimeout:()=>{},Date};
  vm.runInNewContext(code,ctx);
  await new Promise(setImmediate);
  assert.match(rows.innerHTML,/queued/,'initial status fetch');
  assert.equal(requests[0].url,'/projects/4/sources/status');
  assert.equal(requests[0].options.credentials,'same-origin');
  statusResponse='ready';
  refresh.emit('click');
  await new Promise(setImmediate);
  assert.match(rows.innerHTML,/ready/,'manual refresh updates table without reload');
  assert.equal(count.textContent,'1 редакций');
  assert.equal(assignments,0);
  statusResponse='queued';
  form.emit('submit',{preventDefault(){}});
  await new Promise(setImmediate);
  assert.equal(xhrObject.headers['X-Requested-With'],'XMLHttpRequest');
  assert.equal(xhrObject.method,'POST');
  assert.match(uploadNote.textContent,/Зарегистрировано файлов: 1/);
  assert.equal(button.disabled,false,'submit re-enabled');
  assert.equal(picker.value,'','file selection cleared after accepted registration');
  assert.equal(assignments,0,'no full page navigation for HTTP 202');
  assert.match(rows.innerHTML,/queued/);
  console.log('Project sources: manual refresh + 202 upload + live data OK');
}
testUI().catch(e=>{console.error(e);process.exitCode=1});
