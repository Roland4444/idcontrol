// Upload with explicit progress inside embedded Bitrix24 iframe.
(()=>{
'use strict';
const form=document.getElementById('project-sources-form');
if(!form)return;
const btn=form.querySelector('button[type=submit]');
const note=form.querySelector('[role=status]');
const progress=form.querySelector('progress');
form.addEventListener('submit',evt=>{
  const picker=form.querySelector('input[type=file]');
  if(!picker||!picker.files?.length){evt.preventDefault();if(note)note.textContent='Выберите файлы проекта';return;}
  if(!window.XMLHttpRequest||!window.FormData)return;
  evt.preventDefault();if(btn.disabled)return;btn.disabled=true;
  if(progress){progress.hidden=false;progress.value=0;}
  if(note)note.textContent='Передача проектных файлов: 0%';
  const xhr=new XMLHttpRequest();xhr.open('POST',form.action,true);xhr.withCredentials=true;
  xhr.upload.addEventListener('progress',e=>{
    if(!e.lengthComputable||!e.total)return;
    const n=Math.round(100*e.loaded/e.total);
    if(progress)progress.value=n;
    if(note)note.textContent=n<100?`Передача проектных файлов: ${n}%`:'Регистрация файлов, запуск обработки…';
  });
  xhr.addEventListener('load',()=>{
    btn.disabled=false;
    if(xhr.status>=200&&xhr.status<300)location.href=(xhr.responseURL||location.pathname)+'#project-sources';
    else if(note)note.textContent=`Ошибка HTTP ${xhr.status}: ${(xhr.responseText||'').replace(/<[^>]*>/g,' ').slice(0,300)}`;
  });
  xhr.addEventListener('error',()=>{btn.disabled=false;if(note)note.textContent='Ошибка соединения';});
  xhr.send(new FormData(form));
});
})();
