// Offline deterministic smoke tests for iframe login. Run: node scripts/test-bitrix-client.cjs
const fs = require('fs');
const vm = require('vm');
const assert = require('node:assert/strict');
const code = fs.readFileSync('assets/bitrix-client.js', 'utf8');
async function boot({embedded=true, userError=false, fetchStatus=200}={}) {
  const status={textContent:''}, help={textContent:''};
  const requests=[], redirects=[], apiCalls=[];
  let installFinish=0;
  const win={self:{},top:{},location:{replace(path){redirects.push(path)}}};
  if(!embedded)win.top=win.self;
  const sdk={
    install(cb){if(typeof cb !== 'function')throw new Error('install callback missing');sdk._install=cb},
    installFinish(){installFinish++},
    init(cb){cb()},
    callMethod(method, payload, cb){apiCalls.push(method);cb({error:()=>userError?'denied':null,data:()=>({ID:'336',EMAIL:'work@example.org',NAME:'Employee'})})},
    getAuth(){return {domain:'example.bitrix24.ru',access_token:'fake-test-token-for-tests'}}
  };
  const ctx={window:win,document:{getElementById:(id)=>id==='status'?status:help},BX24:sdk,
    setTimeout:()=>42,clearTimeout:()=>{},
    fetch:async (path,params)=>{requests.push({path,params});return {ok:fetchStatus===200,status:fetchStatus,json:async()=>({ok:true})}}};
  vm.runInNewContext(code,ctx,{filename:'bitrix-client.js'});
  await new Promise(resolve=>setImmediate(resolve));
  return {status,help,requests,redirects,apiCalls,sdk,getInstallFinish:()=>installFinish};
}
(async()=>{
  const ok=await boot();
  assert.deepEqual(ok.apiCalls,['user.current']);
  assert.equal(ok.requests.length,1);
  assert.equal(ok.requests[0].path,'/auth/bitrix');
  assert.equal(ok.requests[0].params.credentials,'same-origin');
  const data=JSON.parse(ok.requests[0].params.body);
  assert.deepEqual(Object.keys(data).sort(),['access_token','domain']);
  assert.deepEqual(ok.redirects,['/']);
  ok.sdk._install();
  assert.equal(ok.getInstallFinish(),1);
  const outside=await boot({embedded:false});
  assert.equal(outside.requests.length,0);
  assert.match(outside.status.textContent,/Bitrix24/);
  const failed=await boot({userError:true});
  assert.equal(failed.requests.length,0);
  assert.match(failed.status.textContent,/не вернул/);
  const authFailed=await boot({fetchStatus:403});
  assert.equal(authFailed.redirects.length,0);
  assert.match(authFailed.status.textContent,/HTTP 403/);
  console.log('4 Bitrix bootstrap smoke tests passed');
})().catch(e=>{console.error(e);process.exitCode=1});
