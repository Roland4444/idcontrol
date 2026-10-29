const assert = require('node:assert/strict');
const vm = require('node:vm');
const fs = require('node:fs');
const script = fs.readFileSync(require('node:path').join(__dirname, '../assets/package-actions.js'), 'utf8');

class Form {
  constructor(kind) {
    this.kind = kind; this.events = {};
    this.action = `/packages/123/${kind}`;
    this.dataset = {statusUrl:'/packages/123/scan/status'};
    this.button = {disabled:false,addEventListener(k,fn){this[k]=fn}};
    this.feedback = {textContent:''};
    this.progress = {hidden:true, value:0};
    this.picker = {files:[], focus(){},addEventListener(k,fn){this[k]=fn}};
  }
  querySelector(sel) {
    if (sel.includes('button')) return this.button;
    if (sel.includes('feedback')) return this.feedback;
    if (sel.includes('progress')) return this.progress;
    if (sel.includes('file')) return this.picker;
    return null;
  }
  addEventListener(evt, fn) { this.events[evt] = fn; }
  submit() { this.events.submit({preventDefault(){}}); }
}
function makeContext(upload, scan, fetchImpl) {
  let lastXHR;
  class MockXHR {
    constructor() { lastXHR=this; this.events={}; this.upload={events:{}, addEventListener:(k,f)=>this.upload.events[k]=f}; }
    open(method,url) { this.method=method; this.url=url; }
    addEventListener(k,f) { this.events[k]=f; }
    send(data) { this.sent=data; }
  }
  const context = {
    document:{querySelector: s => s.includes('package-upload') ? upload : scan},
    window:{XMLHttpRequest:MockXHR, FormData:class{}, fetch:fetchImpl, location:{assign(url){context.redirect=url},reload(){context.reloaded=true}}},
    XMLHttpRequest:MockXHR, FormData:class{}, URLSearchParams:class{constructor(){}},
    fetch:fetchImpl, setInterval(){return 1},clearInterval(){}, console
  };
  return {context, get xhr(){return lastXHR}};
}
(async () => {
  const up = new Form('upload');
  const env=makeContext(up,null,null);
  vm.runInNewContext(script,env.context);
  up.submit();
  assert.match(up.feedback.textContent,/выберите файлы/);
  assert.equal(env.xhr,undefined);
  up.picker.files=[{name:'Акт.doc'}];
  up.submit();
  assert.equal(env.xhr.url,'/packages/123/upload');
  env.xhr.upload.events.progress({lengthComputable:true,loaded:50,total:100});
  assert.equal(up.progress.value,50);
  env.xhr.status=403;env.xhr.responseText='Нет доступа';env.xhr.events.load();
  assert.match(up.feedback.textContent,/HTTP 403.*Нет доступа/);
  assert.equal(up.button.disabled,false);

  up.submit();
  env.xhr.status=200; env.xhr.responseURL='/packages/123?notice=uploaded';env.xhr.events.load();
  assert.equal(env.context.redirect,'/packages/123?notice=uploaded');

  console.log('OK: upload validation, percentage, error, success redirect; NAS scanning controls removed');
})().catch(e=>{console.error(e);process.exitCode=1});
