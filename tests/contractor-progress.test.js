// Standalone browser-script smoke tests: `node tests/contractor-progress.test.js`
const vm = require('node:vm');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const code = fs.readFileSync(path.resolve(__dirname, '../assets/contractor-upload.js'), 'utf8');
function el() { return {hidden:true, value:0, textContent:'', dataset:{statusUrl:'/contractor/abc/status'}}; }
async function run(claimed) {
  const ids = new Map(Object.entries({
    'delivery-progress':el(), 'send-status':el(), 'send-bar':el(),
    'processing-stage':el(), 'processing-bar':el(), 'processing-value':el(), 'processing-detail':el(),
  }));
  const button={disabled:false};
  const file={files:[{name:'test.zip'}]};
  const form={action:'/contractor/abc',hidden:false,
    querySelector(sel){return sel.includes('button')?button:file;},
    addEventListener(type,cb){if(type==='submit')this.onSubmit=cb;},
  };
  const timers=[];
  class MockXHR {
    constructor(){this.upload={};}
    open(method,url){assert.equal(method,'POST');assert.equal(url,form.action);}
    setRequestHeader(){}
    send(){
      this.upload.onprogress({lengthComputable:true, loaded:50,total:100});
      assert.match(ids.get('send-status').textContent,/50 %/);
      this.upload.onprogress({lengthComputable:true, loaded:100,total:100});
      this.upload.onload();
      this.status=202;
      this.responseText=JSON.stringify({accepted:true,receipt:'ИД-5-00001',status_url:'/contractor/abc/status'});
      this.onload();
    }
  }
  const scope={
    document:{querySelector:()=>claimed?null:form,getElementById:id=>ids.get(id)||null},
    XMLHttpRequest:MockXHR,FormData:class{constructor(f){assert.equal(f,form);}},
    fetch:async(url,opts)=>{
      assert.equal(url,'/contractor/abc/status');assert.equal(opts.cache,'no-store');
      return {ok:true,json:async()=>({receipt:'ИД-5-00001',status:'pending_review',stage:'completed',percent:100,bytes:4,total_bytes:4,complete:true,failed:false})};
    },
    setInterval:cb=>(timers.push(cb),timers.length),clearInterval:()=>{},console,
  };
  vm.runInNewContext(code,scope);
  if(!claimed) form.onSubmit({preventDefault(){}});
  await new Promise(resolve=>setImmediate(resolve));
  assert.equal(ids.get('processing-bar').value,100);
  assert.match(ids.get('processing-stage').textContent,/успешно/);
  assert.match(ids.get('processing-detail').textContent,/ответственный сотрудник/);
  assert.equal(ids.get('delivery-progress').hidden,false);
  if(!claimed){assert.equal(form.hidden,true);assert.equal(button.disabled,true);}
}
(async()=>{
  await run(false);
  await run(true);
  console.log('Contractor upload UI: POST 202 / browser % / status polling / refresh OK');
})().catch(e=>{console.error(e);process.exitCode=1;});
