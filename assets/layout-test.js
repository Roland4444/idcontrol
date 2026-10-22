
fetch('/api/me',{credentials:'same-origin'}).then(r=>r.ok?r.json():null).then(u=>{if(!u)return;const l=document.getElementById('user-label');if(l)l.textContent=u.name;const a=document.getElementById('admin-tab');if(u.superadmin){if(a)a.hidden=false;document.querySelectorAll('.super-only').forEach(x=>x.hidden=false);}}).catch(()=>{});
(function(){
  async function refresh(el){
    if(document.hidden)return;
    try{
      const response=await fetch(el.dataset.url,{credentials:'same-origin',cache:'no-store'});
      if(!response.ok)throw new Error('HTTP '+response.status);
      const openIds=Array.from(el.querySelectorAll('details[open]')).map(d=>d.dataset.job);
      el.innerHTML=await response.text();
      el.querySelectorAll('details').forEach(d=>{if(openIds.includes(d.dataset.job))d.open=true;});
      const updated=new Set();
      el.querySelectorAll('.job[data-document-id]').forEach(j=>{
        const id=j.dataset.documentId;
        if(updated.has(id))return;
        updated.add(id);
        document.querySelectorAll('.ocr-inline[data-doc="'+id+'"]').forEach(span=>{
          span.textContent=' · OCR-задание: '+j.dataset.jobStatus;
        });
      });
      el.dataset.lastOk=new Date().toLocaleTimeString();
      const indicator=document.getElementById('ocr-poll-state');
      if(indicator)indicator.textContent='Обновлено: '+el.dataset.lastOk;
    }catch(e){
      const indicator=document.getElementById('ocr-poll-state');
      if(indicator)indicator.textContent='Ошибка обновления: '+String(e);
    }
  }
  async function refreshClassification(el){
    if(document.hidden)return;
    try{const r=await fetch(el.dataset.url,{credentials:'same-origin',cache:'no-store'});
      if(r.ok)el.innerHTML=await r.text();
    }catch(e){console.warn('Classification status unavailable',e);}
  }
  const topProgress=document.getElementById('top-progress');
  const coverage=document.querySelector('#summary [role="progressbar"]');
  if(topProgress && document.getElementById('summary')){
    topProgress.textContent=coverage ? 'Комплектность: '+coverage.getAttribute('aria-valuenow')+'%' : 'Комплектность: чек-лист не настроен';
  }
  function navigateToAnchor(fragment, updateUrl, smooth) {
    let id;
    try { id=decodeURIComponent(fragment.replace(/^#/,'')); } catch (_) { return false; }
    const target=document.getElementById(id);
    if(!target)return false;
    const article=target.closest('article');
    const details=article?.querySelector('details.collapsible');
    if(details)details.open=true;
    document.querySelectorAll('article.highlight-section').forEach(el=>el.classList.remove('highlight-section'));
    if(article)article.classList.add('highlight-section');
    document.querySelectorAll('#summary .jump-links a').forEach(a=>{
      a.classList.toggle('active',a.getAttribute('href')==='#'+id || (article && a.getAttribute('href')==='#'+article.id));
    });
    if(updateUrl)history.pushState(null,'','#'+encodeURIComponent(id));
    // Allow the revealed <details> to participate in layout before scrolling.
    requestAnimationFrame(()=>requestAnimationFrame(()=>{
      target.scrollIntoView({behavior:smooth && !window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'smooth' : 'auto',block:'start'});
    }));
    return true;
  }
  document.addEventListener('click',event=>{
    const a=event.target.closest('a[href]');
    if(!a)return;
    const dest=a.getAttribute('href');
    if(!dest || dest==='#' || !dest.startsWith('#'))return;
    if(navigateToAnchor(dest,true,true))event.preventDefault();
  });
  function openDeepLink(){if(location.hash)navigateToAnchor(location.hash,false,false);}
  window.addEventListener('hashchange',openDeepLink);
  window.addEventListener('popstate',openDeepLink);
  openDeepLink();
  document.addEventListener('input', (event)=>{
    const inp=event.target;if(!inp.matches('[data-table-search]'))return;
    const table=document.getElementById(inp.dataset.tableSearch);if(!table)return;
    const q=inp.value.toLocaleLowerCase('ru');
    table.querySelectorAll('tbody tr').forEach(tr=>{tr.hidden=!tr.textContent.toLocaleLowerCase('ru').includes(q);});
  });
  document.addEventListener('change', (event)=>{
    const el=event.target;
    if(el.matches('select[name="kind"]')){
      const form=el.closest('form');const catalog=form?.querySelector('select[name="catalog_code"]');
      if(catalog){catalog.disabled=el.value==='REFERENCE';if(catalog.disabled)catalog.value='';}
    }
    if(el.matches('input[type="file"][multiple]')){
      const counter=document.getElementById('upload-count');if(counter)counter.textContent=`Выбрано файлов: ${el.files.length}`;
    }
  });
  const zone=document.getElementById('upload-drop');
  if(zone){
    const input=zone.querySelector('input[type=file]');
    zone.addEventListener('dragover',e=>{e.preventDefault();zone.classList.add('drag-hover');});
    zone.addEventListener('dragleave',e=>{e.preventDefault();zone.classList.remove('drag-hover');});
    zone.addEventListener('drop',e=>{e.preventDefault();zone.classList.remove('drag-hover');
      if(input && e.dataTransfer?.files?.length){
        input.files=e.dataTransfer.files;
        input.dispatchEvent(new Event('change',{bubbles:true}));
      }
    });
  }
  setInterval(()=>document.querySelectorAll('.classification-live').forEach(refreshClassification),5000);
  document.querySelectorAll('.classification-live').forEach(refreshClassification);
  setInterval(()=>document.querySelectorAll('.ocr-live').forEach(refresh),4000);
  document.querySelectorAll('.ocr-live').forEach(refresh);
})();
