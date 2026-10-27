'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const code = fs.readFileSync(path.join(__dirname, '../assets/ui-version-watch.js'), 'utf8');
const initial = { html:'html-old', js:'js-old', css:'css-old', images:'images-old' };

async function scenario(newManifest, withFailure=false) {
  const calls=[];
  const reloads=[];
  const events={};
  const origin='https://idai.example.net';
  const ctx={
    URL,
    Date,
    window:{
      location:{href:origin+'/projects/7?test=1#upload',replace:(url)=>reloads.push(url)},
      addEventListener:(name,cb)=>{events[name]=cb;}
    },
    document:{
      hidden:false,
      querySelector:(selector)=>{
        const name=/idcontrol-(html|js|css|images)-id/.exec(selector);
        return name?{content:initial[name[1]]}:null;
      },
      addEventListener:(name,cb)=>{events[name]=cb;}
    },
    fetch:(url,opts)=>{
      calls.push({url,opts});
      if(withFailure) return Promise.reject(new Error('offline'));
      return Promise.resolve({ok:true,json:()=>Promise.resolve(newManifest)});
    },
    setInterval:(fn,n)=>{ assert.equal(n,15000); events.interval=fn; }
  };
  vm.runInNewContext(code,ctx);
  // Settle promise chain, including .finally() and timers.
  await new Promise(resolve=>setImmediate(resolve));
  return {calls,reloads,events,ctx};
}
(async()=>{
  const same=await scenario(initial);
  assert.equal(same.reloads.length,0,'unchanged version must not reload');
  assert.equal(same.calls.length,1);
  assert.equal(same.calls[0].opts.cache,'no-store');
  assert.equal(same.calls[0].opts.credentials,'same-origin');

  const changed=await scenario({...initial,css:'css-new'});
  assert.equal(changed.reloads.length,1,'changed CSS must reload');
  const refreshed=new URL(changed.reloads[0]);
  assert.equal(refreshed.pathname,'/projects/7');
  assert.equal(refreshed.searchParams.get('test'),'1');
  assert.equal(refreshed.hash,'#upload');
  assert.match(refreshed.searchParams.get('__ui_rev'),/css-new/);
  changed.events.interval();
  await new Promise(resolve=>setImmediate(resolve));
  assert.equal(changed.reloads.length,1,'reload must only occur once');

  const offline=await scenario(initial,true);
  assert.equal(offline.reloads.length,0,'network failure must not reload');

  // A change discovered on app focus after an uneventful initial check.
  const focus=await scenario(initial);
  focus.ctx.fetch=()=>Promise.resolve({ok:true,json:()=>Promise.resolve({...initial,js:'js-new'})});
  focus.events.focus();
  await new Promise(resolve=>setImmediate(resolve));
  assert.equal(focus.reloads.length,1,'focus check must catch a newer JS release');
  console.log('UI auto-update: unchanged, changed CSS, no duplicate reload, offline, focus -> PASS');
})().catch(err=>{console.error(err);process.exitCode=1;});
