// Run: node tests/employee-picker.test.js
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
class Elem {
  constructor(tag) {
    this.tagName = tag;
    this.children = [];
    this.options = tag === 'select' ? [] : undefined;
    this.listeners = {};
    this.dataset = {};
    this.style = {};
    this.hidden = false;
    this.value = '';
    this.elements = {namedItem: n => this.children.find(c => c.name === n)};
  }
  appendChild(el) {this.children.push(el); if (this.options) this.options.push(el); return el;}
  append(...els) {els.forEach(e => this.appendChild(e));}
  insertAdjacentElement(_, el) {this.after = el;}
  setAttribute(k,v) {this[k] = v;}
  addEventListener(k,cb) {(this.listeners[k] ||= []).push(cb);}
  replaceChildren() {this.children = [];}
  get textContent() {return this._text || '';}
  set textContent(s) {this._text = s;}
  scrollIntoView() {}
  setCustomValidity(s) {this.invalidMessage = s;}
  reportValidity() {return !this.invalidMessage;}
  focus() {}
}
const form = new Elem('form');
const select = new Elem('select');
select.form = form;
select.appendChild(Object.assign(new Elem('option'), {value: '', textContent:'Выберите сотрудника'}));
const named = new Elem('span');named.dataset.employeeId = '12';
let filters = null;
let fetched = false;
let authCalls = 0;
const pages = [
  [{ID: 12, ACTIVE: true, USER_TYPE:'employee', NAME:'Иван', LAST_NAME:'Петров'},
   {ID: 13, ACTIVE: true, USER_TYPE:'extranet', NAME:'Гость', LAST_NAME:'Петров'},
   {ID: 14, ACTIVE: false, USER_TYPE:'employee', NAME:'Бывший', LAST_NAME:'Петров'},
   {ID: 15, ACTIVE: true, USER_TYPE:'email', NAME:'Почтовый', LAST_NAME:'Петров'}],
  [{ID: 16, ACTIVE: 'Y', USER_TYPE:'employee', NAME:'Мария', LAST_NAME:'Иванова'},
   {ID: 17, ACTIVE: false, USER_TYPE:'guest', NAME:'Гость', LAST_NAME:'Иванова'}]
];
let position = 0;
const BX24 = {
  init(cb) {cb();},
  getAuth() {authCalls++;return {access_token:'a-real-mock-token'};},
  callMethod(name,params,cb) {
    assert.equal(name,'user.get');
    filters = params.FILTER;
    const emit = () => cb({error:()=>null,data:()=>pages[position],more:()=>position<pages.length-1,next:()=>{position++;emit();}});
    emit();
  }
};
const document = {
  querySelectorAll(s) {
    if (s === 'select.employee-picker') return [select];
    if (s === '[data-employee-id]') return [named];
    return [];
  },
  createElement(tag) {return new Elem(tag);}
};
const window={self:{},top:{}};
const ctx={document,window,BX24,fetch(){fetched=true;return Promise.resolve({})},setTimeout:()=>0};
const js=fs.readFileSync(path.join(__dirname,'../assets/employee-picker.html.js'),'utf8');
vm.runInNewContext(js,ctx);
assert.equal(fetched,false, 'Production mode must not merge stale local users');
assert.equal(filters.ACTIVE,true);
assert.equal(filters.USER_TYPE,'employee');
assert.equal(named.textContent,'Петров Иван');
const input=select.after.children[0], popup=select.after.children[1];
input.value='Петр Иван';
input.listeners.input[0]();
assert.deepEqual(popup.children.map(x=>x.textContent),['Петров Иван']);
assert.ok(!popup.children.some(x=>x.textContent.includes('Гость')));
assert.ok(!popup.children.some(x=>x.textContent.includes('Бывший')));
popup.children[0].listeners.click[0]();
assert.equal(select.value,'12');
assert.equal(input.value,'Петров Иван');
assert.equal(popup.hidden,true);
form.listeners.submit[0]({preventDefault(){throw Error('Valid selection must submit');}});
assert.equal(form.elements.namedItem('bitrix_token').value,'a-real-mock-token');
assert.equal(authCalls,1);
console.log('OK: REST filtering, multi-page, excluding guests/inactive, FIO search, selection and server verification token');
