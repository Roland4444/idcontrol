const assert = require('assert');
const fs = require('fs');
const vm = require('vm');
const script = fs.readFileSync(require('path').join(__dirname, '../assets/contractor-link-copy.js'), 'utf8');

async function run(scenario) {
  const handlers = {};
  const input = {
    value: '', dataset: {path:'/contractor/abc123'}, focus() {this.focused=true;},
    select(){this.selected=true;},setSelectionRange(){this.ranged=true;}
  };
  const button = {disabled:false, addEventListener(event,fn){handlers[event]=fn;}};
  const status = {textContent:''};
  const textarea = {value:'',style:{}, setAttribute(){},focus(){},select(){},remove(){}};
  const doc = {
    getElementById(id){return {'contractor-url':input,'copy-contractor-url':button,'contractor-copy-status':status}[id];},
    createElement(){return textarea;},
    body:{appendChild(el){assert.strictEqual(el,textarea);}},
    execCommand(){return scenario.execCommand === undefined ? false : scenario.execCommand;}
  };
  const clipboard = scenario.clipboard === 'success' ? {writeText: async v => {assert.strictEqual(v,'https://id.example/contractor/abc123');}}
    : scenario.clipboard === 'reject' ? {writeText: async () => {throw Error('denied');}}
    : undefined;
  const ctx = {document:doc,navigator:{clipboard},window:{location:{href:'https://id.example/packages/42'}}, URL};
  vm.runInNewContext(script, ctx);
  assert.strictEqual(input.value,'https://id.example/contractor/abc123');
  await handlers.click();
  assert.strictEqual(button.disabled,false);
  return {input,status,textarea};
}
(async () => {
 let result=await run({clipboard:'success'});
 assert.strictEqual(result.status.textContent,'Ссылка скопирована');
 result=await run({clipboard:'reject',execCommand:true});
 assert.strictEqual(result.status.textContent,'Ссылка скопирована');
 result=await run({clipboard:null,execCommand:false});
 assert.match(result.status.textContent,/Ctrl\+C/);
 assert.strictEqual(result.input.selected,true);
 console.log('OK: modern clipboard, iframe fallback, manual copy fallback');
})().catch(error => {console.error(error);process.exitCode=1;});
