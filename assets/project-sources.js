// Project library: background processing status without full page refresh.
// Handles large uploads in a Bitrix24 iframe; 100% means transfer, not registration.
(()=>{
'use strict';
const section=document.getElementById('project-sources');
if(!section)return;
const form=document.getElementById('project-sources-form');
const uploadButton=form?.querySelector('button[type=submit]');
const uploadNote=form?.querySelector('[role=status]');
const progress=form?.querySelector('progress');
const picker=form?.querySelector('input[type=file]');
const refreshButton=document.getElementById('project-sources-refresh');
const refreshNote=document.getElementById('project-sources-refresh-note');
const rows=document.getElementById('project-sources-rows');
const count=document.getElementById('project-sources-count');
const url=refreshButton?.dataset.statusUrl;
if(!url||!rows||!refreshButton)return;

let pollTimer=null;
let requestInFlight=null;
let lastPending=0;
let uploadInFlight=false;
const POLL_MS=3000;
function notice(message){if(refreshNote)refreshNote.textContent=message;}
function schedule(){
  if(pollTimer!==null){clearTimeout(pollTimer);pollTimer=null;}
  if(lastPending>0||uploadInFlight)pollTimer=setTimeout(()=>refresh(false),POLL_MS);
}
async function refresh(manual=false){
  if(requestInFlight)return requestInFlight;
  requestInFlight=(async()=>{
    if(manual)notice('Обновление статусов…');
    refreshButton.disabled=true;
    try{
      const response=await fetch(url,{credentials:'same-origin',cache:'no-store',headers:{Accept:'application/json'}});
      if(!response.ok)throw new Error(`HTTP ${response.status}`);
      if(!(response.headers.get('content-type')||'').includes('application/json'))throw new Error('Вместо статусов сервер вернул страницу входа');
      const data=await response.json();
      if(typeof data.rows_html!=='string'||!Number.isInteger(data.pending)||!Number.isInteger(data.total))throw new Error('Неизвестный формат ответа сервера');
      rows.innerHTML=data.rows_html;
      if(count)count.textContent=`${data.total} редакций`;
      lastPending=data.pending;
      const stamp=new Date().toLocaleTimeString('ru-RU');
      notice(`Обновлено ${stamp} · готово: ${data.ready}, в обработке: ${data.pending}, ошибок: ${data.errors}`);
      if(data.pending===0 && !uploadInFlight && progress && progress.value===100 && uploadNote){
        uploadNote.textContent=data.errors>0?'Обработка завершена с ошибками. Причина показана в таблице.':'Загрузка зарегистрирована, обработка завершена. Результаты в таблице.';
      }
      return data;
    }catch(error){
      notice(`Не удалось обновить статусы: ${error.message}`);
      return null;
    }finally{
      refreshButton.disabled=false;
    }
  })();
  try{return await requestInFlight;}
  finally{requestInFlight=null;schedule();}
}
refreshButton.addEventListener('click',()=>{void refresh(true);});
// The first refresh discovers unfinished work after ordinary navigation/reload.
void refresh(false);

if(!form||!uploadButton)return;
form.addEventListener('submit',evt=>{
  if(!picker||!picker.files?.length){evt.preventDefault();if(uploadNote)uploadNote.textContent='Выберите файлы проекта';return;}
  if(!window.XMLHttpRequest||!window.FormData)return;
  evt.preventDefault();
  if(uploadInFlight)return;
  uploadInFlight=true;
  uploadButton.disabled=true;
  if(progress){progress.hidden=false;progress.value=0;}
  if(uploadNote)uploadNote.textContent='Передача проектных файлов: 0%';
  const xhr=new XMLHttpRequest();
  xhr.open('POST',form.action,true);
  xhr.withCredentials=true;
  xhr.setRequestHeader('X-Requested-With','XMLHttpRequest');
  xhr.upload.addEventListener('progress',e=>{
    if(!e.lengthComputable||!e.total)return;
    const n=Math.round(100*e.loaded/e.total);
    if(progress)progress.value=n;
    if(uploadNote)uploadNote.textContent=n<100?`Передача проектных файлов: ${n}%`:'Файлы переданы. Сервер регистрирует их; это может занять время для больших PDF. Статусы можно обновлять ниже.';
  });
  function finish(){uploadInFlight=false;uploadButton.disabled=false;schedule();}
  xhr.addEventListener('load',()=>{
    finish();
    if(xhr.status===202){
      let data={};try{data=JSON.parse(xhr.responseText||'{}');}catch(_){}
      const n=Number(data.queued)||0;
      const skipped=Number(data.unchanged)||0;
      if(progress)progress.value=100;
      if(uploadNote)uploadNote.textContent=n>0?`Зарегистрировано файлов: ${n}. Обработка на Synology и распознавание идут в фоне.`:skipped>0?'Файлы не изменились и уже присутствуют в библиотеке.':'Новых файлов для регистрации нет.';
      picker.value='';
      // A status fetch started before registration may have been coalesced.
      // Schedule one follow-up to discover the newly queued rows reliably.
      void refresh(true).then(()=>{
        if(n>0 && lastPending===0){
          if(pollTimer!==null)clearTimeout(pollTimer);
          pollTimer=setTimeout(()=>refresh(false),POLL_MS);
        }
      });
    }else if(xhr.status>=200&&xhr.status<300){
      // Compatible with an older binary that still answers with a redirect.
      window.location.assign(`${window.location.pathname}#project-sources`);
    }else{
      const message=(xhr.responseText||'').replace(/<[^>]*>/g,' ').replace(/\s+/g,' ').slice(0,300);
      if(uploadNote)uploadNote.textContent=`Регистрация не удалась: HTTP ${xhr.status}. ${message}`;
      void refresh(true);
    }
  });
  xhr.addEventListener('error',()=>{
    finish();
    if(uploadNote)uploadNote.textContent='Ошибка соединения. Проверьте список ниже, прежде чем повторять загрузку.';
    void refresh(true);
  });
  xhr.addEventListener('abort',()=>{
    finish();
    if(uploadNote)uploadNote.textContent='Передача прервана. Проверьте список зарегистрированных файлов.';
    void refresh(true);
  });
  xhr.send(new FormData(form));
  schedule();
});
})();
