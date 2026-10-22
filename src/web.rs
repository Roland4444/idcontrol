use crate::{access,portal,catalog::Catalog,classification,config::Config,engine,models::{Finding,Package},norms,ocr_jobs,repo,reports,scanner};
use anyhow::Result;
use axum::{
    body::{Body,Bytes},
    extract::{DefaultBodyLimit,Multipart,Path,Query,State},Extension,
    http::{header,StatusCode},
    middleware,
    response::{Html,IntoResponse,Redirect,Response},
    routing::{get,post}, Form,Json,Router,
};
use serde::Deserialize;
use sqlx::PgPool;
use std::{collections::{HashMap,HashSet},io::Write,path::PathBuf,sync::{Arc,RwLock,atomic::{AtomicBool,Ordering}}};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
use tracing::error;

#[derive(Default,Clone)]
pub struct ScanStatus {
    pub running:bool,
    pub last_at:Option<chrono::DateTime<chrono::Utc>>,
    pub message:String,
}

pub struct AppState {
    pub debug_login:bool,
    pub cfg:Config, pub db:PgPool, pub catalog:Catalog,
    pub nas:crate::synology::SynologyClient, pub synology_connected:AtomicBool,
    pub scan_lock:Mutex<()>, pub csrf:String,
    pub scan_status:RwLock<ScanStatus>,
}
pub type Shared=Arc<AppState>;

pub struct WebError {status:StatusCode, detail:String}
impl WebError {
    pub(crate) fn bad(s:impl Into<String>)->Self{Self{status:StatusCode::BAD_REQUEST,detail:s.into()}}
    pub(crate) fn not_found()->Self{Self{status:StatusCode::NOT_FOUND,detail:"Not found".into()}}
    pub(crate) fn forbidden()->Self{Self{status:StatusCode::FORBIDDEN,detail:"CSRF validation failed".into()}}
}
impl From<anyhow::Error> for WebError {
    fn from(err:anyhow::Error)->Self{error!("Request failed: {err:#}");Self{status:StatusCode::INTERNAL_SERVER_ERROR,detail:"Internal server error (check server logs)".into()}}
}
impl From<sqlx::Error> for WebError { fn from(err:sqlx::Error)->Self {Self::from(anyhow::Error::from(err))} }
impl From<std::io::Error> for WebError { fn from(err:std::io::Error)->Self {Self::from(anyhow::Error::from(err))} }
impl From<http::Error> for WebError { fn from(err:http::Error)->Self {Self::from(anyhow::Error::from(err))} }
impl From<serde_json::Error> for WebError { fn from(err:serde_json::Error)->Self {Self::from(anyhow::Error::from(err))} }
impl From<tokio::task::JoinError> for WebError { fn from(err:tokio::task::JoinError)->Self {Self::from(anyhow::Error::from(err))} }
impl IntoResponse for WebError {fn into_response(self)->Response{(self.status,self.detail).into_response()}}
type WebResult<T>=std::result::Result<T,WebError>;
fn h(s:&str)->String {html_escape::encode_safe(s).to_string()}
pub(crate) fn hidden(csrf:&str)->String {format!("<input type=\"hidden\" name=\"csrf\" value=\"{}\">",h(csrf))}
pub(crate) fn verify(csrf:&str,state:&Shared)->WebResult<()> {
    if bool::from(csrf.as_bytes().ct_eq(state.csrf.as_bytes())) {Ok(())}else{Err(WebError::forbidden())}
}
pub(crate) fn layout(title:&str,body:String)->Html<String>{
    Html(format!(r#"<!DOCTYPE html><html lang="ru"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>{title} — Контроль ИД</title>
<style>body{{background:#f4f7fa;color:#1d334a;font:15px system-ui,Arial;margin:0}}header{{padding:17px 5%;background:#223851;color:white}}header a{{color:white;margin-right:24px;text-decoration:none}}main{{max-width:1550px;margin:28px auto;padding:0 16px}}article{{background:white;border:1px solid #d8e0e8;border-radius:10px;padding:18px;margin:18px 0;overflow-x:auto}}table{{width:100%;border-collapse:collapse}}td,th{{padding:9px;border-bottom:1px solid #dae1e8;text-align:left;vertical-align:top}}th{{background:#eef3f7}}input,select,textarea,button{{padding:8px;border-radius:5px;border:1px solid #bbc7d2;font:inherit;margin:3px}}button{{background:#224a69;color:white;border:0;cursor:pointer}}a{{color:#116caa}}small,.muted{{color:#536778}}.warn{{color:#8b4d17}}.ok{{color:#386e4c}}.row{{display:flex;gap:12px;flex-wrap:wrap;align-items:center}}.chips{{display:flex;gap:8px;flex-wrap:wrap}}.chip{{background:#e8eff5;border-radius:15px;padding:5px 9px}}.status{{font-weight:600}}.job{{padding:12px;margin:10px 0;border:1px solid #dce4ed;border-radius:8px;background:#fafcfe}}.job-running{{border-left:4px solid #2379a1}}.job-failed{{border-left:4px solid #c34d4d}}.job-done{{border-left:4px solid #45936e}}.job-partial{{border-left:4px solid #c99a3a}}progress{{width:220px;max-width:100%;height:14px}}.toolbox{{display:flex;gap:10px;flex-wrap:wrap;align-items:center}}.toolbox form{{display:inline-flex}}.secondary{{background:#e8f0f7;color:#153b56}}.warnbutton{{background:#a0532c}}
:root{{color-scheme:light}}*{{box-sizing:border-box}}html{{scroll-behavior:smooth}}body{{line-height:1.45}}main{{max-width:1440px}}article{{scroll-margin-top:76px}}header{{position:sticky;top:0;z-index:10;box-shadow:0 1px 6px #071c2d24}}header a{{display:inline-block;padding:3px 0}}.overview{{background:#fff;border:1px solid #d8e0e8;border-radius:12px;padding:23px;margin:18px 0;scroll-margin-top:74px}}.metrics{{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:12px;margin-top:15px}}.metric{{padding:15px;border:1px solid #dae4ed;border-radius:9px;background:#fbfdff;min-width:0}}.metric strong{{font-size:24px;display:block;color:#123b54}}.jump-links{{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:11px;margin-top:18px}}.jump-links a{{display:block;border:1px solid #d2e0e9;border-radius:9px;padding:13px;text-decoration:none;background:#f8fbfe;font-weight:600}}.jump-links a small{{display:block;font-weight:400;margin-top:4px}}.jump-links a:hover{{background:#eaf4fb}}details.collapsible>summary{{font-size:19px;font-weight:650;cursor:pointer;padding:6px 2px;list-style:revert}}details.collapsible>summary::marker{{color:#28719e}}details.collapsible>div{{padding:12px 0}}details.collapsible:not([open]){{padding-bottom:12px}}.summarybar{{height:10px;background:#e6edf3;border-radius:8px;overflow:hidden;margin-top:12px}}.summarybar span{{background:#28719e;display:block;height:100%}}.notice{{border-left:4px solid #288258;padding:10px 14px;background:#edf8f0;margin:15px 0;border-radius:6px}}.privacy{{border:1px solid #d6ab69;background:#fffbf2;border-radius:8px;padding:14px;margin:12px 0}}.uploadfile{{max-width:100%}}.filefilter{{width:min(480px,100%)}}.filelist td{{max-width:530px;overflow-wrap:anywhere}}.filelist tr:target{{outline:2px solid #2183a5}}.minor{{font-size:13px}}.edit-row{{border:1px solid #e0e7ec;padding:10px;border-radius:6px;margin:8px 0}}.sticky-info{{display:inline-block;font-size:13px;color:#d3e7f4;margin-left:16px}}.resultlink{{text-decoration:none}}.buttonlink{{display:inline-block;background:#e8f0f7;color:#153b56;text-decoration:none;border-radius:6px;padding:9px}}.sr-only{{position:absolute;left:-9999px}}@media(max-width:800px){{.metrics{{grid-template-columns:repeat(2,minmax(0,1fr))}}.jump-links{{grid-template-columns:repeat(2,minmax(0,1fr))}}header a{{margin-right:8px}}article{{padding:12px}}td,th{{padding:6px}}}}@media(max-width:480px){{.jump-links{{grid-template-columns:1fr}}}}.attention-panel{{background:#fff;border:1px solid #d7e2eb;border-radius:12px;padding:22px;margin:18px 0 28px}}.attention-list{{display:grid;grid-template-columns:repeat(3,minmax(0,1fr));gap:12px}}.attention-item{{border:1px solid #d9e3eb;border-radius:9px;padding:16px;background:#fafcfe;min-width:0}}.attention-item strong{{display:block;font-size:21px;margin-bottom:6px}}.attention-item a{{display:inline-block;margin-top:8px}}.detail-sections{{border-top:3px solid #d7e2eb;margin-top:38px;padding-top:22px;scroll-margin-top:90px}}.detail-sections>h2{{margin:8px 0}}.detail-group{{margin:27px 0 36px}}.detail-group>h3{{color:#244965;margin:0 0 6px}}.detail-group>p{{margin:0 0 12px;color:#536778}}.detail-group article{{margin:10px 0}}article.highlight-section{{outline:3px solid #2b82a5;outline-offset:3px;box-shadow:0 8px 22px #2b82a51c}}.jump-links a.active{{border-color:#28719e;background:#dceefa}}.backline{{text-align:right;margin:0 0 14px}}.backline a{{font-weight:600}}.detail-intro{{color:#536778}}@media(max-width:850px){{.attention-list{{grid-template-columns:1fr}}}}
{portal_style}</style></head>
<body><header><a class=brand href="/"><strong>RES</strong><span>DEVELOPMENT</span></a><a class="nav-link active" href="/">Проверка</a><a class="nav-link super-only" hidden href="/catalog/errors">Типовые ошибки</a><a class="nav-link super-only" hidden href="/norms">Нормативные правила</a><a class="nav-link super-only" hidden href="/diagnostics">Диагностика / OCR</a><div class=nav-spacer></div><a id="admin-tab" class="nav-link nav-right" href="/administration" hidden>⚙ Администрирование</a><span id=user-label class=user-label></span><span class=sticky-info id=top-progress></span></header><main>{body}</main><script>
fetch('/api/me',{{credentials:'same-origin'}}).then(r=>r.ok?r.json():null).then(u=>{{if(!u)return;const l=document.getElementById('user-label');if(l)l.textContent=u.name;const a=document.getElementById('admin-tab');if(u.superadmin){{if(a)a.hidden=false;document.querySelectorAll('.super-only').forEach(x=>x.hidden=false);}}}}).catch(()=>{{}});
(function(){{
  async function refresh(el){{
    if(document.hidden)return;
    try{{
      const response=await fetch(el.dataset.url,{{credentials:'same-origin',cache:'no-store'}});
      if(!response.ok)throw new Error('HTTP '+response.status);
      const openIds=Array.from(el.querySelectorAll('details[open]')).map(d=>d.dataset.job);
      el.innerHTML=await response.text();
      el.querySelectorAll('details').forEach(d=>{{if(openIds.includes(d.dataset.job))d.open=true;}});
      const updated=new Set();
      el.querySelectorAll('.job[data-document-id]').forEach(j=>{{
        const id=j.dataset.documentId;
        if(updated.has(id))return;
        updated.add(id);
        document.querySelectorAll('.ocr-inline[data-doc="'+id+'"]').forEach(span=>{{
          span.textContent=' · OCR-задание: '+j.dataset.jobStatus;
        }});
      }});
      el.dataset.lastOk=new Date().toLocaleTimeString();
      const indicator=document.getElementById('ocr-poll-state');
      if(indicator)indicator.textContent='Обновлено: '+el.dataset.lastOk;
    }}catch(e){{
      const indicator=document.getElementById('ocr-poll-state');
      if(indicator)indicator.textContent='Ошибка обновления: '+String(e);
    }}
  }}
  async function refreshClassification(el){{
    if(document.hidden)return;
    try{{const r=await fetch(el.dataset.url,{{credentials:'same-origin',cache:'no-store'}});
      if(r.ok)el.innerHTML=await r.text();
    }}catch(e){{console.warn('Classification status unavailable',e);}}
  }}
  const topProgress=document.getElementById('top-progress');
  const coverage=document.querySelector('#summary [role="progressbar"]');
  if(topProgress && document.getElementById('summary')){{
    topProgress.textContent=coverage ? 'Комплектность: '+coverage.getAttribute('aria-valuenow')+'%' : 'Комплектность: чек-лист не настроен';
  }}
  function navigateToAnchor(fragment, updateUrl, smooth) {{
    let id;
    try {{ id=decodeURIComponent(fragment.replace(/^#/,'')); }} catch (_) {{ return false; }}
    const target=document.getElementById(id);
    if(!target)return false;
    const article=target.closest('article');
    const details=article?.querySelector('details.collapsible');
    if(details)details.open=true;
    document.querySelectorAll('article.highlight-section').forEach(el=>el.classList.remove('highlight-section'));
    if(article)article.classList.add('highlight-section');
    document.querySelectorAll('#summary .jump-links a').forEach(a=>{{
      a.classList.toggle('active',a.getAttribute('href')==='#'+id || (article && a.getAttribute('href')==='#'+article.id));
    }});
    if(updateUrl)history.pushState(null,'','#'+encodeURIComponent(id));
    // Allow the revealed <details> to participate in layout before scrolling.
    requestAnimationFrame(()=>requestAnimationFrame(()=>{{
      target.scrollIntoView({{behavior:smooth && !window.matchMedia('(prefers-reduced-motion: reduce)').matches ? 'smooth' : 'auto',block:'start'}});
    }}));
    return true;
  }}
  document.addEventListener('click',event=>{{
    const a=event.target.closest('a[href]');
    if(!a)return;
    const dest=a.getAttribute('href');
    if(!dest || dest==='#' || !dest.startsWith('#'))return;
    if(navigateToAnchor(dest,true,true))event.preventDefault();
  }});
  function openDeepLink(){{if(location.hash)navigateToAnchor(location.hash,false,false);}}
  window.addEventListener('hashchange',openDeepLink);
  window.addEventListener('popstate',openDeepLink);
  openDeepLink();
  document.addEventListener('input', (event)=>{{
    const inp=event.target;if(!inp.matches('[data-table-search]'))return;
    const table=document.getElementById(inp.dataset.tableSearch);if(!table)return;
    const q=inp.value.toLocaleLowerCase('ru');
    table.querySelectorAll('tbody tr').forEach(tr=>{{tr.hidden=!tr.textContent.toLocaleLowerCase('ru').includes(q);}});
  }});
  document.addEventListener('change', (event)=>{{
    const el=event.target;
    if(el.matches('select[name="kind"]')){{
      const form=el.closest('form');const catalog=form?.querySelector('select[name="catalog_code"]');
      if(catalog){{catalog.disabled=el.value==='REFERENCE';if(catalog.disabled)catalog.value='';}}
    }}
    if(el.matches('input[type="file"][multiple]')){{
      const counter=document.getElementById('upload-count');if(counter)counter.textContent=`Выбрано файлов: ${{el.files.length}}`;
    }}
  }});
  const zone=document.getElementById('upload-drop');
  if(zone){{
    const input=zone.querySelector('input[type=file]');
    zone.addEventListener('dragover',e=>{{e.preventDefault();zone.classList.add('drag-hover');}});
    zone.addEventListener('dragleave',e=>{{e.preventDefault();zone.classList.remove('drag-hover');}});
    zone.addEventListener('drop',e=>{{e.preventDefault();zone.classList.remove('drag-hover');
      if(input && e.dataTransfer?.files?.length){{
        input.files=e.dataTransfer.files;
        input.dispatchEvent(new Event('change',{{bubbles:true}}));
      }}
    }});
  }}
  setInterval(()=>document.querySelectorAll('.classification-live').forEach(refreshClassification),5000);
  document.querySelectorAll('.classification-live').forEach(refreshClassification);
  setInterval(()=>document.querySelectorAll('.ocr-live').forEach(refresh),4000);
  document.querySelectorAll('.ocr-live').forEach(refresh);
}})();
</script></body></html>"#,
        title=h(title),body=body,portal_style=include_str!("../assets/portal.css")))
}

pub fn router(state:Shared)->Router{
    Router::new()
        .route("/",get(portal::projects))
        .route("/bitrix",get(access::bitrix_bootstrap))
        .route("/auth/bitrix",post(access::login))
        .route("/auth/logout",post(access::logout))
        .route("/api/me",get(whoami))
        .route("/assets/projects/{filename}",get(portal::project_image))
        .route("/projects/{id}",get(portal::project))
        .route("/projects/{id}/works",post(portal::create_work))
        .route("/projects/{id}/works/all",post(portal::create_all))
        .route("/projects/{id}/assign",post(portal::assign_work))
        .route("/administration",get(portal::administration))
        .route("/administration/managers",post(portal::assign_manager))
        .route("/health",get(health))
        .route("/diagnostics",get(diagnostics))
        .route("/api/ocr/jobs",get(ocr_jobs_json))
        .route("/api/ocr/status",get(ocr_status_json))
        .route("/ocr/jobs/fragment",get(ocr_jobs_fragment))
        .route("/ocr/jobs/{id}/cancel",post(cancel_ocr_job))
        .route("/documents/{id}/reprocess",post(reprocess_document))
        .route("/packages/{id}/reprocess-errors",post(reprocess_package_errors))
        .route("/packages/{id}/reprocess-all",post(reprocess_package_all))
        .route("/catalog/errors",get(errors))
        .route("/norms",get(norms_view).post(create_norm_rule))
        .route("/norms/{id}/status",post(norm_rule_status))

        .route("/packages/{id}",get(package_view))
        .route("/packages/{id}/catalog",post(set_catalog))
        .route("/packages/{id}/checklist/custom",post(create_custom_checklist))
        .route("/packages/{id}/checklist/custom/{item_id}",post(edit_custom_checklist))
        .route("/packages/{id}/scan",post(manual_scan))
        .route("/packages/{id}/ai-consent",post(set_ai_consent))
        .route("/packages/{id}/ai-check",post(queue_ai_check))
        .route("/ai-suggestions/{id}/review",post(review_ai_suggestion))
        .route("/packages/{id}/upload",post(upload))
        .route("/documents/{id}/classify",post(classify))
        .route("/packages/{id}/classification/provider",post(set_classify_provider))
        .route("/packages/{id}/classification/all",post(queue_classify_all))
        .route("/packages/{id}/classification/unclassified",post(queue_classify_unclassified))
        .route("/packages/{id}/classification/fragment",get(classification_fragment))
        .route("/documents/{id}/ai-classify",post(queue_classify_one))
        .route("/documents/{id}/ai-classify/confirm",post(confirm_classification))
        .route("/documents/{id}/ai-classify/reject",post(reject_classification))
        .route("/documents/{id}/download",get(download))
        .route("/findings/{id}/review",post(review))
        .route("/packages/{id}/report/{fmt}",get(report))
        .layer(DefaultBodyLimit::max((state.cfg.max_file_mb.saturating_mul(20).saturating_mul(1024*1024).saturating_add(1024*1024)) as usize))
        .layer(middleware::from_fn_with_state(state.clone(),access::authorize))
        .with_state(state)
}

async fn whoami(Extension(actor):Extension<access::Principal>)->Json<serde_json::Value>{
    Json(serde_json::json!({"name":actor.name,"id":actor.user_id,"superadmin":actor.superuser,"debug":actor.debug}))
}

#[derive(sqlx::FromRow)]
struct CustomChecklist {id:i64,code:String,title:String,required:bool,note:String}
#[derive(Deserialize,Default)]struct RunQuery{run_id:Option<i64>,notice:Option<String>,doc:Option<i64>}
async fn package_view(State(st):State<Shared>,Extension(actor):Extension<access::Principal>,Path(id):Path<i64>,Query(params):Query<RunQuery>)->WebResult<Html<String>>{
    let pkg=repo::package(&st.db,id).await.map_err(|_|WebError::not_found())?;
    let projects=sqlx::query_as::<_,crate::models::Project>("SELECT id,name FROM projects WHERE id=$1")
        .bind(pkg.project_id).fetch_one(&st.db).await?;
    let docs=repo::documents(&st.db,id).await?;
    let mut current=HashMap::new();
    for doc in &docs{current.entry(doc.relative_path.clone()).or_insert(doc);}
    let runs=repo::runs(&st.db,id).await?;
    let selected=match params.run_id {Some(run_id)=>runs.iter().find(|r|r.id==run_id).ok_or_else(WebError::not_found).map(Some)?,None=>runs.first()};
    let findings=if let Some(run)=selected{repo::findings(&st.db,run.id).await?}else{vec![]};
    let norm_checks=if let Some(run)=selected{norms::checks_for_run(&st.db,run.id).await?}else{vec![]};
    let selected_codes:HashSet<String>=serde_json::from_str::<Vec<String>>(&pkg.required_codes_json).unwrap_or_default().into_iter().collect();
    let custom_items:Vec<CustomChecklist>=sqlx::query_as("SELECT id,code,title,required,note FROM package_custom_checklist WHERE package_id=$1 AND active=true ORDER BY id")
        .bind(id).fetch_all(&st.db).await?;
    let mut latest:Vec<_>=current.values().filter(|d|d.is_present).collect();
    latest.sort_by(|a,b|a.relative_path.cmp(&b.relative_path));
    let id_docs=latest.iter().filter(|d|d.source_kind=="ID").count();
    let ref_docs=latest.iter().filter(|d|d.source_kind=="REFERENCE").count();
    let required_codes:HashSet<String>=selected_codes.iter().cloned().chain(custom_items.iter().filter(|i|i.required).map(|i|i.code.clone())).collect();
    let attached_codes:HashSet<String>=latest.iter().filter_map(|d|d.catalog_code.clone()).collect();
    let completed=required_codes.intersection(&attached_codes).count();
    let unresolved=findings.iter().filter(|f|f.review_status=="pending" || f.review_status=="confirmed").count();
    let nonclassified=latest.iter().filter(|d|d.source_kind=="ID" && d.catalog_code.is_none()).count();
    let pct=if required_codes.is_empty(){None}else{Some((completed*100/required_codes.len()).min(100))};
    let actor_role=access::package_role(&st,&actor,id).await.unwrap_or_else(||"viewer".into());
    let mut sidebar=String::new();
    let sibling_packages:Vec<Package>=sqlx::query_as("SELECT id,project_id,name,catalog_name,required_codes_json FROM packages WHERE project_id=$1 ORDER BY name")
        .bind(pkg.project_id).fetch_all(&st.db).await?;
    for p in &sibling_packages {
        if access::package_role(&st,&actor,p.id).await.is_none(){continue;}
        sidebar.push_str(&format!("<a class='sidebar-work {}' href='/packages/{}'>▧ {}</a>",if p.id==id{"active"}else{""},p.id,h(&p.name)));
    }
    let (address,slug)=portal::project_info(&projects.name).unwrap_or(("Адрес не указан","tetris"));
    let mut body=format!(r##"<div class="crumb"><a href="/">Выбор объектов</a> › <a href="/projects/{}">{}</a> › {}</div>
      <section class="package-hero" style="--hero-image:url('/assets/projects/{}.webp')"><div><h1>{}</h1><p>⌖ {} · {} · Комплект исполнительной документации</p></div></section>
      <div class="package-shell"><aside class="package-sidebar"><div class="sidebar-project"><img src="/assets/projects/{}.webp" alt="Объект"><div><strong>{}</strong><small>{}</small></div></div><h3>Комплекты исполнительной документации</h3>{}</aside><div class="package-content role-{}">
      <nav class="package-tabs"><a href="#upload">Загрузка и проверка</a><a href="#documents">Список документов</a><a href="#checks">Результаты проверки</a><a href="#checklist">Чек-лист</a><a href="#ocr">История / OCR</a></nav>"##,
      pkg.project_id,h(&projects.name),h(&pkg.name),slug,h(&pkg.name),h(address),h(&projects.name),slug,h(&projects.name),h(address),sidebar,h(&actor_role));
    if let Some(notice)=params.notice.as_deref() {
        let msg=match notice {"classified"=>"Тип документа сохранён, комплект перепроверен.","uploaded"=>"Файлы загружены. Результаты обработки доступны ниже.","checklist"=>"Изменения чек-листа сохранены, комплект перепроверен.",_=>""};
        if !msg.is_empty(){body.push_str(&format!("<p class=notice role=status>{msg} {}</p>",params.doc.map(|d|format!("Документ #{d}")).unwrap_or_default()));}
    }
    let coverage=match pct {Some(n)=>format!("{n}%"),None=>"Не настроен".to_string()};
    body.push_str(&format!("<section class=overview id=summary><h2>Результат проверки комплекта</h2><p class=muted>Прогресс показывает покрытие обязательных позиций файлами, <b>не</b> соответствие ГОСТ/СП. Рассматривается только выбранный комплект.</p><div class=metrics><div class=metric>Файлы ИД<strong>{id_docs}</strong><small>ПД/РД: {ref_docs}</small></div><div class=metric>Чек-лист<strong>{completed}/{}</strong><small>Обязательных позиций</small></div><div class=metric>Замечания<strong>{unresolved}</strong><small>Требуют рассмотрения</small></div><div class=metric>Не привязаны<strong>{nonclassified}</strong><small>Файлов ИД</small></div></div>",required_codes.len()));
    body.push_str(&format!("<p><b>Комплектность: {coverage}</b></p>"));
    if let Some(n)=pct {body.push_str(&format!("<div class=summarybar role=progressbar aria-valuemin=0 aria-valuemax=100 aria-valuenow={n} aria-label='Покрытие чек-листа'><span style='width:{n}%'></span></div>"));}
    body.push_str("<nav class=jump-links aria-label='Разделы комплекта'><a href=#checks>Замечания<small>Результаты и решения инженера</small></a><a href=#documents>Документы<small>Файлы, типы и вложения</small></a><a href=#checklist>Чек-лист<small>Изменение обязательных позиций</small></a><a href=#upload>Загрузить<small>Несколько файлов одной операцией</small></a><a href=#classification>ИИ-классификация<small>Предложения модели и подтверждение</small></a><a href=#ocr>Обработка и прогресс<small>Фоновые задания OCR</small></a><a href=#norms-check>Нормативные правила<small>Подробные результаты</small></a></nav></section>");
    // A compact action-oriented summary stays above ALL detailed menus.
    let missing=required_codes.len().saturating_sub(completed);
    body.push_str("<section class=attention-panel aria-labelledby=attention-title><h2 id=attention-title>Что требует внимания</h2><p class=muted>Ниже перечислены только сводные результаты. Подробные таблицы находятся внизу страницы: нажмите на ссылку, чтобы открыть нужный раздел.</p><div class=attention-list>");
    if required_codes.is_empty() {
        body.push_str("<div class=attention-item><strong>Чек-лист не настроен</strong><span>Выберите обязательные позиции, чтобы оценить комплектность.</span><br><a href=#checklist>Настроить чек-лист ↓</a></div>");
    } else {
        body.push_str(&format!("<div class=attention-item><strong>{missing} отсутствуют</strong><span>Обязательные позиции без приложенных документов</span><br><a href=#checklist>Открыть чек-лист ↓</a></div>"));
    }
    body.push_str(&format!("<div class=attention-item><strong>{unresolved} замечаний</strong><span>Ожидают решения или подтверждены инженером</span><br><a href=#checks>Открыть замечания ↓</a></div>"));
    body.push_str(&format!("<div class=attention-item><strong>{nonclassified} не привязаны</strong><span>Файлы ИД без позиции чек-листа</span><br><a href=#documents>Перейти к документам ↓</a></div>"));
    body.push_str("</div></section>");
    // Build detail panels separately; they are emitted only at the BOTTOM of the page.
    let mut panels:HashMap<&'static str,String>=HashMap::new();
    let mut panel_html=String::new();
    // Start primary sections. Detailed tables are intentionally folded away.
    panel_html.push_str(&format!("<article id='upload'><details class=collapsible><summary>Загрузка документов</summary><div><p>Выберите сразу несколько файлов. Все они будут прикреплены к одной позиции чек-листа, если указана позиция.</p><form action='/packages/{id}/upload' method=post enctype=multipart/form-data>{}<label>Тип <select name=kind><option value=ID>Исполнительная документация</option><option value=REFERENCE>ПД/РД (локально, без отправки внешнему ИИ)</option></select></label><label>Позиция чек-листа <select name=catalog_code><option value=''>Не назначена</option>",hidden(&st.csrf)));
    if let Some(wt)=pkg.catalog_name.as_deref().and_then(|n|st.catalog.work_type(n)) {
        for item in &wt.items {panel_html.push_str(&format!("<option value='{}'>{} — {}</option>",h(&item.code),h(&item.code),h(&item.title)));}
    }
    for item in &custom_items {panel_html.push_str(&format!("<option value='{}'>{}</option>",h(&item.code),h(&item.title)));}
    panel_html.push_str("</select></label><div class=drop-zone id=upload-drop><strong class=upload-mark>⇧</strong><h3>Перетащите файлы сюда или выберите на компьютере</h3><p class=muted>PDF, DOCX, XLSX, JPG, PNG, TIFF, BMP, GIF, WebP · до 20 файлов (DWG пока не поддерживается)</p><input class=uploadfile type=file name=file multiple required accept='.pdf,.docx,.xlsx,.jpg,.jpeg,.png,.tif,.tiff,.bmp,.gif,.webp'><small id=upload-count>Файлы не выбраны</small></div><button class=primary>Загрузить файлы</button></form>");
    panel_html.push_str(&format!("<form action='/packages/{id}/scan' method=post>{}<button class=secondary>Просканировать Synology</button></form></div></details></article>",hidden(&st.csrf)));
    panels.insert("upload",std::mem::take(&mut panel_html));
    panel_html.push_str(&format!("<article id=checklist><details class=collapsible><summary>Чек-лист и вид работ</summary><div><form method=post action=\"/packages/{id}/catalog\">{}<select name=work_type>",hidden(&st.csrf)));
    for wt in &st.catalog.work_types{let sel=if pkg.catalog_name.as_deref()==Some(wt.name.as_str()){ "selected" }else{""};panel_html.push_str(&format!("<option {sel} value=\"{}\">{}</option>",h(&wt.name),h(&wt.name)));}
    panel_html.push_str("</select><p class=muted>Отметьте документы, обязательность которых утвердил инженер. До утверждения ни один пункт не считается автоматически обязательным.</p>");

    if let Some(wt)=pkg.catalog_name.as_deref().and_then(|n|st.catalog.work_type(n)){
        for item in &wt.items{
            let checked=if selected_codes.contains(&item.code){"checked"}else{""};
            panel_html.push_str(&format!("<p><label><input type=checkbox name=required_codes value=\"{}\" {checked}> <b>{}</b> {} <small>({})</small></label></p>",h(&item.code),h(&item.code),h(&item.title),h(&item.norm_reference)));
        }
    }
    panel_html.push_str("<button>Сохранить шаблон и обязательность</button></form><h3>Дополнительные позиции этого комплекта</h3>");
    for item in &custom_items {
        panel_html.push_str(&format!("<form class=edit-row method=post action='/packages/{id}/checklist/custom/{}'>{}<label>Название <input name=title required maxlength=300 size=47 value=\"{}\"></label><label>Примечание <input name=note maxlength=500 size=28 value=\"{}\"></label><label><input type=checkbox name=required value=yes {}> Обязательный</label><button>Сохранить</button><button class=warnbutton name=remove value=yes onclick=\"return confirm('Убрать позицию из чек-листа? Приложенные документы останутся в архиве.')\">Убрать</button></form>",item.id,hidden(&st.csrf),h(&item.title),h(&item.note),if item.required {"checked"}else{""}));
    }
    panel_html.push_str(&format!("<form method=post action='/packages/{id}/checklist/custom'>{}<label>Добавить позицию <input name=title required minlength=2 maxlength=300 placeholder='Например: Протокол испытания сваи' size=48></label><label>Примечание <input name=note maxlength=500 placeholder='Необязательно'></label><label><input type=checkbox name=required value=yes checked> Обязательная</label><button>Добавить</button></form></div></details></article>",hidden(&st.csrf)));
    panels.insert("checklist",std::mem::take(&mut panel_html));
    // AI review is explicitly opted in per package; never triggered by Synology polling.
    let ai_consent:(bool,)=sqlx::query_as("SELECT ai_consent FROM packages WHERE id=$1")
        .bind(id).fetch_one(&st.db).await?;
    panel_html.push_str("<article id=ai-check><details class=collapsible><summary>Внешняя ИИ-проверка (ограничена)</summary><div><p class=privacy><b>Не передавайте ПД/РД внешним ИИ!</b> На сервере передача проектной/рабочей документации в VibeCode, GigaCowork и другие внешние сервисы запрещена. ИИ-классификация доступна только для файлов ИД.</p>");
    panel_html.push_str(&format!("<p>Режим ИИ: <b>{}</b>. Передавать можно только разрешённые выдержки ИД; ПД/РД обрабатываются исключительно локально.</p>",h(&st.cfg.ai_mode)));
    panel_html.push_str(&format!("<form method=post action=\"/packages/{id}/ai-consent\">{}<label><input type=checkbox name=consent value=yes {}> Разрешаю передавать выдержки из исполнительной документации для ИИ-классификации. ПД/РД никогда не передавать</label><button>Сохранить разрешение</button></form>",hidden(&st.csrf),if ai_consent.0 {"checked"} else {""}));
    panel_html.push_str("<p class=warn>Внешняя сверка ИД с ПД/РД отключена, так как для неё потребовалась бы передача ПД/РД внешнему провайдеру.</p>");
    panel_html.push_str("<p class=muted>ИИ не правит документы, не утверждает ГОСТ и не создаёт официальные замечания. Результаты — предложения для инженера.</p>");
    let jobs:Vec<(i64,String,String,i32)>=sqlx::query_as(
       "SELECT id,provider,status,coalesce(suggestions_created,0) FROM ai_jobs WHERE package_id=$1 ORDER BY id DESC LIMIT 8")
       .bind(id).fetch_all(&st.db).await?;
    if !jobs.is_empty() {
        panel_html.push_str("<p><b>Задания ИИ:</b> ");
        for (job_id,provider,status,count) in jobs {
            panel_html.push_str(&format!(" #{} [{}: {}, {} предложений] ",job_id,h(&provider),h(&status),count));
        }
        panel_html.push_str("</p>");
    }
    let suggestions:Vec<AiSuggestionRow>=sqlx::query_as(
        "SELECT id,document_id,reference_document_id,source_quote,reference_quote,description,proposed_action,review_status,comment FROM ai_suggestions WHERE package_id=$1 ORDER BY id DESC LIMIT 150")
        .bind(id).fetch_all(&st.db).await?;
    if !suggestions.is_empty() {
        panel_html.push_str("<table><tr><th>Документы/цитаты</th><th>Предложение ИИ</th><th>Решение инженера</th></tr>");
        for s in suggestions {
            panel_html.push_str(&format!("<tr><td>ИД #{} / РД #{}<p>{}</p><p>{}</p></td><td>{}<p><small>{}</small></p></td><td><form method=post action=\"/ai-suggestions/{}/review\">{}<select name=status>",s.document_id,s.reference_document_id.unwrap_or(0),h(&s.source_quote),h(&s.reference_quote),h(&s.description),h(&s.proposed_action),s.id,hidden(&st.csrf)));
            for (value,label) in [("pending","На рассмотрении"),("confirmed","Подтверждено"),("rejected","Отклонено")] {
                let sel=if s.review_status==value {"selected"}else{""};
                panel_html.push_str(&format!("<option value={value} {sel}>{label}</option>"));
            }
            panel_html.push_str(&format!("</select><input name=comment maxlength=2000 placeholder=Комментарий value=\"{}\"><button>Сохранить</button></form></td></tr>",h(&s.comment)));
        }
        panel_html.push_str("</table>");
    }
    panel_html.push_str("</div></details></article>");
    panels.insert("ai-check",std::mem::take(&mut panel_html));
    // Classification provider is selected PER PACKAGE, independent of the OCR provider.
    let provider_options:Option<(String,bool)>=sqlx::query_as("SELECT provider,auto_queue FROM classification_settings WHERE package_id=$1")
        .bind(id).fetch_optional(&st.db).await?;
    let (provider,auto_queue)=provider_options.unwrap_or_else(||("vibecode".into(),false));
    panel_html.push_str("<article id=classification><details class=collapsible><summary>ИИ-классификация документов</summary><div><p class=muted>После OCR анализируется распознанный текст. Это не нормативная проверка. ИИ предлагает тип документа; инженер подтверждает или отклоняет предложение. Оригиналы не изменяются.</p>");
    panel_html.push_str(&format!("<form method=post action='/packages/{id}/classification/provider'>{}<label>Языковая модель <select name=provider>",hidden(&st.csrf)));
    for (v,label) in [("vibecode","Bitrix VibeCode"),("gigacowork","GigaCowork (через корпоративный адаптер)")] {
        panel_html.push_str(&format!("<option value='{v}' {}>{label}</option>",if provider==v {"selected"}else{""}));
    }
    panel_html.push_str(&format!("</select></label><label><input type=checkbox name=auto_queue value=yes {}> Автоматически ставить в очередь новые распознанные документы (только при наличии согласия)</label><button>Сохранить настройки</button></form>", if auto_queue {"checked"} else {""}));
    panel_html.push_str(&format!("<p>VibeCode: {}. GigaCowork: {}. <b>Выбран:</b> {}.</p>",
       if classification::configured(&st.cfg,"vibecode") {"ключ настроен"} else {"нет VIBE_OCR_KEY"},
       if classification::configured(&st.cfg,"gigacowork") {"адаптер настроен"} else {"нет CLASSIFY_COWORK_URL"},
       h(&provider)));
    if !ai_consent.0 {
        panel_html.push_str("<p class=warn>Перед внешней ИИ-классификацией включи согласие на передачу текстовых фрагментов в блоке выше.</p>");
    }
    if !classification::configured(&st.cfg,&provider) {
        panel_html.push_str("<p class=warn>Выбранный провайдер пока не настроен: запросы не отправляются.</p>");
    }
    let disabled=if ai_consent.0 && classification::configured(&st.cfg,&provider) {""} else {"disabled"};
    panel_html.push_str(&format!("<div class=toolbox><form method=post action='/packages/{id}/classification/unclassified'>{}<button {disabled}>Классифицировать неопределённые</button></form><form method=post action='/packages/{id}/classification/all'>{}<button class=secondary {disabled}>Переклассифицировать все</button></form></div>",hidden(&st.csrf),hidden(&st.csrf)));
    panel_html.push_str(&format!("<div class='classification-live' data-url='/packages/{id}/classification/fragment'>Загрузка журнала классификации...</div>"));
    panel_html.push_str("<p class=muted>В GigaCowork нет подтверждённого публичного REST API: нужен собственный адаптер по контракту docs/GIGACOWORK_CLASSIFIER_ADAPTER_RU.md. Ключи только в .env; никакого автоматического переключения или отправки документов без согласия.</p></div></details></article>");
    panels.insert("classification",std::mem::take(&mut panel_html));
    let classification_rows:Vec<(i64,String,String,f64,String,String,serde_json::Value)>=sqlx::query_as(
        "SELECT document_id,provider,predicted_type,confidence,explanation,review_status,evidence_json FROM classification_results WHERE document_id IN (SELECT id FROM documents WHERE package_id=$1)")
        .bind(id).fetch_all(&st.db).await?;
    let classification_by_document:HashMap<i64,(String,String,f64,String,String,serde_json::Value)>=classification_rows
        .into_iter().map(|(did,p,t,c,e,r,q)|(did,(p,t,c,e,r,q))).collect();
    panel_html.push_str(&format!("<article id=ocr><details class=collapsible><summary>Повторное распознавание и задания</summary><div><p class=muted>Работает фоновая очередь. Статусы обновляются каждые 4 секунды без перезагрузки страницы. Оригиналы и версии файлов не изменяются.</p><div class=toolbox><form method=post action='/packages/{id}/reprocess-errors'>{}<button>Распознать все с ошибками</button></form><form method=post action='/packages/{id}/reprocess-all'>{}<button class=secondary onclick=\"return confirm('Повторно обработать ВСЕ документы комплекта? При OCR_MODE=vibecode возможны платные запросы VibeCode.')\">Перераспознать весь комплект</button></form><a href='/diagnostics'>Журнал и диагностика</a> <small id=ocr-poll-state></small></div><div class='ocr-live' data-url='/ocr/jobs/fragment?package_id={id}'>Загрузка очереди...</div></div></details></article>",hidden(&st.csrf),hidden(&st.csrf)));
    panels.insert("ocr",std::mem::take(&mut panel_html));
    panel_html.push_str("<article id=documents><details class=collapsible><summary>Документы и вложения</summary><div><p>Документы, назначенные одной позиции чек-листа, считаются подтверждающими её вложениями. Можно прикреплять любое количество файлов.</p><label>Поиск по файлам и типам <input type=search class=filefilter data-table-search=doc-table placeholder='Название, тип, статус'></label><table class=filelist id=doc-table><thead><tr><th>Файл</th><th>Тип и распознавание</th><th>Версия</th><th>Действия</th></tr></thead><tbody>");
    let mut shown:Vec<_>=current.values().collect();
    shown.sort_by(|a,b| a.source_kind.cmp(&b.source_kind)
        .then(a.catalog_code.cmp(&b.catalog_code)).then(a.relative_path.cmp(&b.relative_path)));
    let mut last_group=String::new();
    for doc in shown{
        let group=if doc.source_kind=="REFERENCE" {"ПД/РД — только локальная обработка".to_string()}
        else if let Some(code)=&doc.catalog_code {
            let title=pkg.catalog_name.as_deref().and_then(|n|st.catalog.work_type(n))
                .and_then(|wt|wt.items.iter().find(|i|i.code==*code)).map(|i|i.title.as_str())
                .or_else(||custom_items.iter().find(|i|i.code==*code).map(|i|i.title.as_str()));
            format!("{} — {}",code,title.unwrap_or("Позиция чек-листа"))
        }else{"Без привязки к чек-листу".to_string()};
        if group!=last_group {
            panel_html.push_str(&format!("<tr class=group-row><th colspan=4>{}</th></tr>",h(&group)));
            last_group=group;
        }
        let delete_msg=if !doc.is_present{" <span class=warn>(отсутствует на NAS)</span>"}else{""};
        panel_html.push_str(&format!("<tr id=doc-{}><td>{}{}<br><small>{}</small></td><td>{}<br><small>{}</small></td><td>v{}</td><td><a href=\"/documents/{}/download\">Оригинал</a>",doc.id,h(&doc.relative_path),delete_msg,h(&doc.sha256[..12]),h(doc.document_type.as_deref().unwrap_or("не классифицирован")),h(&doc.recognition_status),doc.version,doc.id));
        panel_html.push_str(&format!(" <span class='ocr-inline' data-doc='{}'></span>",doc.id));
        if let Some((ai_provider,ai_label,score,explanation,review_status,evidence))=classification_by_document.get(&doc.id) {
            panel_html.push_str(&format!("<div class=job><small>ИИ ({}) — <b>{}</b> · уверенность {:.0}% · {}</small><div><small>{}</small></div>",
                h(ai_provider),h(ai_label),(*score)*100.0,h(review_status),h(explanation)));
            if let Some(items)=evidence.as_array() {
                for quote in items.iter().filter_map(|v|v.as_str()).take(3){panel_html.push_str(&format!("<div><small>Цитата: {}</small></div>",h(quote)));}
            }
            if review_status=="suggested" {
                panel_html.push_str(&format!("<form method=post action='/documents/{}/ai-classify/confirm'>{}<button>Подтвердить тип ИИ</button></form><form method=post action='/documents/{}/ai-classify/reject'>{}<button class=secondary>Отклонить</button></form>",doc.id,hidden(&st.csrf),doc.id,hidden(&st.csrf)));
            }
            panel_html.push_str("</div>");
        }
        if doc.source_kind=="ID" {
            panel_html.push_str(&format!("<form method=post action='/documents/{}/ai-classify'>{}<button class=secondary>ИИ: повторить классификацию</button></form>",doc.id,hidden(&st.csrf)));
        }
        if doc.source_kind=="ID"{
            panel_html.push_str(&format!("<form method=post action=\"/documents/{}/classify\">{}<select name=catalog_code><option value=\"\">Не определён</option>",doc.id,hidden(&st.csrf)));
            if let Some(wt)=pkg.catalog_name.as_deref().and_then(|n|st.catalog.work_type(n)){
                for item in &wt.items{let sel=if doc.catalog_code.as_deref()==Some(item.code.as_str()){"selected"}else{""};panel_html.push_str(&format!("<option {sel} value=\"{}\">{} — {}</option>",h(&item.code),h(&item.code),h(&item.title)));}
            }
            for item in &custom_items {
                let sel=if doc.catalog_code.as_deref()==Some(item.code.as_str()){"selected"}else{""};
                panel_html.push_str(&format!("<option {sel} value=\"{}\">{} — {}</option>",h(&item.code),h(&item.code),h(&item.title)));
            }
            panel_html.push_str("</select><button>Сохранить тип</button></form>");
        }
        panel_html.push_str(&format!("<form method=post action='/documents/{}/reprocess'>{}<button class=secondary>Повторить OCR</button></form>",doc.id,hidden(&st.csrf)));
        panel_html.push_str("</td></tr>");
    }
    panel_html.push_str("</tbody></table></div></details></article>");
    panels.insert("documents",std::mem::take(&mut panel_html));
    panel_html.push_str("<article id=checks><details class=collapsible><summary>Проверки и замечания</summary><div><div class=row>");
    for run in &runs{panel_html.push_str(&format!("<a class=chip href=\"/packages/{id}?run_id={}#checks\">Проверка #{} — {}</a>",run.id,run.id,run.started_at.format("%d.%m.%Y %H:%M")));}
    panel_html.push_str("</div>");
    if let Some(run)=selected{
        panel_html.push_str(&format!("<p><a href=\"/packages/{id}/report/xlsx?run_id={}\">XLSX</a> · <a href=\"/packages/{id}/report/docx?run_id={}\">DOCX</a> · <a href=\"/packages/{id}/report/pdf?run_id={}\">PDF</a> — официальная выгрузка только подтверждённых/устранённых замечаний. <a href=\"/packages/{id}/report/xlsx?run_id={}&draft=true\">Черновик всех</a></p>",run.id,run.id,run.id,run.id));
    }
    panel_html.push_str("<table><tr><th>Блок и код</th><th>Источник и описание</th><th>Результат</th><th>Решение инженера</th></tr>");
    for f in findings{
        panel_html.push_str(&format!("<tr><td>{} / {}</td><td><small>{}</small><p>{}</p>{}</td><td class=status>{}</td><td><form method=post action=\"/findings/{}/review\">{}<select name=status>",f.block,h(&f.rule_code),h(&f.source_location),h(&f.description),if f.reference.is_empty(){"".to_string()}else{format!("<small>{}</small>",h(&f.reference))},if f.result=="mismatch"{"Выявлено несоответствие"}else{"Требуется ручная проверка"},f.id,hidden(&st.csrf)));
        for (value,label) in [("pending","На рассмотрении"),("confirmed","Подтверждено"),("rejected","Отклонено"),("resolved","Устранено")]{
            let sel=if f.review_status==value{"selected"}else{""};panel_html.push_str(&format!("<option value={value} {sel}>{label}</option>"));
        }
        panel_html.push_str(&format!("</select><input name=comment placeholder=Комментарий value=\"{}\"><button>Сохранить</button></form></td></tr>",h(&f.comment)));
    }
    panel_html.push_str("</table><p class=muted>Отсутствие замечаний не подтверждает соответствие всем строительным нормам. Применяются только утверждённые правила; НТД не придумываются.</p></div></details></article>");
    panels.insert("checks",std::mem::take(&mut panel_html));
    panel_html.push_str("<article id=norms-check><details class=collapsible><summary>Проверки утверждённых нормативных правил</summary><div><p class=muted>«Найдено подтверждение» означает обнаружение заданного текста, а не полное соответствие ГОСТ/СП. Настроить правила: <a href=/norms>НТД и пункты</a>.</p><table><tr><th>Документ / правило</th><th>Норматив и источник</th><th>Результат</th></tr>");
    for check in &norm_checks{
        let label=match check.result.as_str(){"evidence_found"=>"Найдено подтверждение в тексте","mismatch"=>"Вероятное несоответствие","manual"=>"Требуется проверка инженера",_=>"Неизвестно"};
        panel_html.push_str(&format!("<tr><td>Документ #{} / правило #{}</td><td>{}<br><small>{}</small></td><td><b>{}</b><p>{}</p></td></tr>",check.document_id,check.rule_id,h(&check.normative_basis),h(&check.source_reference),label,h(&check.details)));
    }
    panel_html.push_str("</table></div></details></article>");
    panels.insert("norms-check",std::mem::take(&mut panel_html));
    // The upload panel is shown directly after the summary for operational staff.
    if let Some(upload_panel)=panels.remove("upload") {
        body.push_str("<section class='main-upload-panel'>");
        body.push_str(&upload_panel.replacen("<details class=collapsible>","<details class=collapsible open>",1));
        body.push_str("</section>");
    }
    body.push_str("<section class=detail-sections id=detail-sections aria-labelledby=details-title><h2 id=details-title>Подробные разделы</h2><p class=detail-intro>Все таблицы и настройки расположены здесь. По умолчанию они свёрнуты. Выбери нужную проверку в сводке наверху — откроется соответствующий раздел ниже.</p>");
    for (title,description,ids) in [
        ("Результаты проверок","Замечания инженеру и утверждённые нормативные правила",vec!["checks","norms-check"]),
        ("Документы и комплект","Загруженные файлы, привязка к позициям, редактирование и загрузка",vec!["documents","checklist"]),
        ("Распознавание и ИИ","Предложения классификатора, очередь OCR и настройки внешнего ИИ",vec!["classification","ocr","ai-check"]),
    ] {
        body.push_str(&format!("<div class=detail-group><h3>{title}</h3><p>{description}</p>"));
        for key in ids {
            if let Some(panel)=panels.remove(key) {
                // Every panel has a way back to the overview without a long manual scroll.
                let panel=panel.replacen("<div>","<div><p class=backline><a href=#summary>↑ Вернуться к сводке</a></p>",1);
                body.push_str(&panel);
            }
        }
        body.push_str("</div>");
    }
    body.push_str("</section>");
    if docs.len()>current.len(){body.push_str(&format!("<p>Предыдущие версии: {} (сохраняются в архиве и доступны по ID).</p>",docs.len()-current.len()));}
    body.push_str("</div></div>");
    Ok(layout("Комплект ИД",body))
}

/// Normative rules must be entered from a source the organisation possesses and explicitly approved.
async fn norms_view(State(st):State<Shared>)->WebResult<Html<String>> {
    let rules=norms::list_rules(&st.db).await?;
    let mut body=String::from("<h1>Нормативные правила (ГОСТ, СП, приказы)</h1><p>Здесь регистрируются <b>проверенные специалистом</b> требования конкретной редакции НТД. Ссылки из исходного ТЗ не считаются автоматически утверждёнными. Пустая база правил означает, что нормативная проверка не пройдена.</p>");
    body.push_str(&format!("<article><h2>Добавить правило на утверждение</h2><form method=post action=/norms>{}",hidden(&st.csrf)));
    body.push_str(r#"<p>Вид работ <select name=work_type><option value="">Все виды работ</option>"#);
    for wt in &st.catalog.work_types{body.push_str(&format!("<option value=\"{}\">{}</option>",h(&wt.name),h(&wt.name)));}
    body.push_str("</select> Код документа по справочнику (необязательно; например 1.4): <input name=document_code placeholder=1.4 maxlength=30></p>");
    body.push_str("<p><input name=norm_name required maxlength=250 placeholder=\"Название ГОСТ / СП / приказа\"> <input name=norm_edition required maxlength=80 placeholder=\"Редакция / год\"> <input name=norm_clause required maxlength=100 placeholder=\"Пункт / приложение НТД\"></p>");
    body.push_str("<p><input name=source_reference required maxlength=500 style=width:80% placeholder=\"Источник: утверждённый PDF / ссылка на файл с нормой\"></p>");
    body.push_str("<p><input name=description required maxlength=500 style=width:80% placeholder=\"Что именно проверяем и почему правило применимо\"></p>");
    body.push_str("<p>Тип проверки: <select name=check_kind><option value=required_phrase>Наличие обязательного текста/реквизита</option><option value=forbidden_phrase>Недопустимый текст/устаревшая ссылка</option><option value=required_pattern>Обязательный реквизит по регулярному выражению</option><option value=manual>Требует инженерной сверки</option></select></p>");
    body.push_str("<p><input name=expected_text maxlength=300 style=width:80% placeholder=\"Точный текст для поиска (для ручной проверки не нужен)\"></p>");
    body.push_str("<button>Создать черновик правила</button></form><p class=muted>Поиск фразы проверяет только присутствие/отсутствие текста. Автоматической правовой экспертизы и анализа геометрии здесь нет.</p></article>");
    body.push_str("<article><h2>Каталог правил</h2><table><tr><th>Правило</th><th>НТД и источник</th><th>Условие</th><th>Состояние</th></tr>");
    for rule in &rules {
        body.push_str(&format!("<tr><td><b>#{} v{}</b> {}<p><small>Вид работ: {}, код: {}</small></p></td><td>{}<p><small>Источник: {}</small></p></td><td>{}<p><small>Текст: {}</small></p></td><td>{}",rule.id,rule.version,h(&rule.description),h(if rule.work_type.is_empty(){"все"}else{&rule.work_type}),h(if rule.document_code.is_empty(){"все"}else{&rule.document_code}),h(&norms::basis(rule)),h(&rule.source_reference),h(&rule.check_kind),h(&rule.expected_text),h(&rule.approval_status)));
        if rule.approval_status!="approved" && rule.approval_status!="revoked" {
            body.push_str(&format!("<form method=post action=\"/norms/{}/status\">{}<input type=hidden name=status value=approved><button>Утвердить</button></form>",rule.id,hidden(&st.csrf)));
        }else if rule.approval_status=="approved" {
            body.push_str(&format!("<p><small>Утвердил: {}</small></p><form method=post action=\"/norms/{}/status\">{}<input type=hidden name=status value=revoked><button>Отозвать</button></form>",h(rule.approved_by.as_deref().unwrap_or("неизвестно")),rule.id,hidden(&st.csrf)));
        }
        body.push_str("</td></tr>");
    }
    body.push_str("</table><p>Редактирование утверждённого правила запрещено: создайте новую версию новым правилом и отзовите старую. Проверки прошлых периодов сохраняются.</p></article>");
    Ok(layout("Нормативные правила",body))
}
#[derive(Deserialize)]
struct NormForm {
    csrf:String, work_type:String, document_code:String, norm_name:String, norm_edition:String,
    norm_clause:String, source_reference:String, description:String, check_kind:String, expected_text:String,
}
async fn create_norm_rule(State(st):State<Shared>,Form(form):Form<NormForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    let filled=[(&form.norm_name,250usize),(&form.norm_edition,80),(&form.norm_clause,100),(&form.source_reference,500),(&form.description,500)];
    if filled.iter().any(|(s,max)|s.trim().is_empty() || s.len()>*max || s.chars().any(char::is_control)) {
        return Err(WebError::bad("Укажите нормативный документ, редакцию, пункт, источник и описание (без управляющих символов)."));
    }
    if !["required_phrase","forbidden_phrase","required_pattern","manual"].contains(&form.check_kind.as_str()) {
        return Err(WebError::bad("Неизвестный тип правила"));
    }
    if form.expected_text.len()>300 || (form.check_kind!="manual" && form.expected_text.trim().chars().count()<3) {
        return Err(WebError::bad("Нужен точный проверяемый текст (минимум 3 символа)."));
    }
    if form.check_kind=="required_pattern" && regex::Regex::new(&form.expected_text).is_err(){
        return Err(WebError::bad("Неверное регулярное выражение (используйте синтаксис Rust regex)."));
    }
    if !form.work_type.is_empty() && st.catalog.work_type(&form.work_type).is_none() {
        return Err(WebError::bad("Неизвестный вид работ"));
    }
    if !form.document_code.is_empty() && (form.work_type.is_empty() || st.catalog.item(&form.work_type,&form.document_code).is_none()){
        return Err(WebError::bad("Код документа должен существовать в выбранном виде работ"));
    }
    let new:(i64,)=sqlx::query_as("INSERT INTO normative_rules(work_type,document_code,norm_name,norm_edition,norm_clause,source_reference,description,check_kind,expected_text,version) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,(SELECT COALESCE(MAX(version),0)+1 FROM normative_rules WHERE work_type=$1 AND document_code=$2 AND norm_name=$3 AND norm_clause=$5)) RETURNING id")
        .bind(&form.work_type).bind(&form.document_code).bind(form.norm_name.trim()).bind(form.norm_edition.trim())
        .bind(form.norm_clause.trim()).bind(form.source_reference.trim()).bind(form.description.trim())
        .bind(&form.check_kind).bind(form.expected_text.trim()).fetch_one(&st.db).await?;
    repo::audit(&st.db,&st.cfg.app_user,"create_norm_rule",&format!("id={}",new.0)).await?;
    Ok(Redirect::to("/norms"))
}
#[derive(Deserialize)]struct NormStatusForm{csrf:String,status:String}
async fn norm_rule_status(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<NormStatusForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    if !["approved","revoked"].contains(&form.status.as_str()) {return Err(WebError::bad("Неизвестный статус"));}
    let count=if form.status=="approved"{
        sqlx::query("UPDATE normative_rules SET approval_status='approved',approved_by=$1,approved_at=now() WHERE id=$2 AND approval_status='draft'")
            .bind(&st.cfg.app_user).bind(id).execute(&st.db).await?.rows_affected()
    }else{
        sqlx::query("UPDATE normative_rules SET approval_status='revoked' WHERE id=$1 AND approval_status='approved'")
            .bind(id).execute(&st.db).await?.rows_affected()
    };
    if count!=1 {return Err(WebError::bad("Правило не найдено или уже изменило статус"));}
    repo::audit(&st.db,&st.cfg.app_user,"norm_rule_status",&format!("id={id} status={}",form.status)).await?;
    Ok(Redirect::to("/norms"))
}

async fn errors(State(st):State<Shared>)->WebResult<Html<String>>{
    let mut body=String::from("<h1>Справочник 34 типовых ошибок</h1><article><table><tr><th>Код</th><th>Ошибка</th><th>Алгоритм</th><th>Обработка</th></tr>");
    for err in &st.catalog.errors{body.push_str(&format!("<tr><td>{}</td><td>{}<p><small>{}</small></p></td><td>{}</td><td>{}</td></tr>",h(&err.code),h(&err.title),h(&err.description),h(&err.algorithm),h(&err.check_type)));}
    body.push_str("</table></article>");Ok(layout("Частые ошибки",body))
}

async fn set_catalog(State(st):State<Shared>,Path(id):Path<i64>,body:Bytes)->WebResult<Redirect>{
    let mut work=None;let mut csrf=None;let mut selected=HashSet::new();
    for (key,value) in url::form_urlencoded::parse(&body){
        match key.as_ref(){"csrf"=>csrf=Some(value.into_owned()),"work_type"=>work=Some(value.into_owned()),"required_codes"=>{selected.insert(value.into_owned());},_=>{}}
    }
    verify(csrf.as_deref().unwrap_or(""),&st)?;
    let wt=st.catalog.work_type(work.as_deref().unwrap_or("")).ok_or_else(||WebError::bad("Unknown work type"))?;
    let valid:HashSet<_>=wt.items.iter().map(|i|i.code.as_str()).collect();
    if selected.iter().any(|code|!valid.contains(code.as_str())){return Err(WebError::bad("Invalid checklist entry"));}
    let _=repo::package(&st.db,id).await.map_err(|_|WebError::not_found())?;
    let mut codes:Vec<_>=selected.into_iter().collect();codes.sort();
    sqlx::query("UPDATE packages SET catalog_name=$1,required_codes_json=$2 WHERE id=$3")
        .bind(&wt.name).bind(serde_json::to_string(&codes)?).bind(id).execute(&st.db).await?;
    engine::analyze(&st.db,&st.catalog,id,"manual").await?;
    repo::audit(&st.db,&st.cfg.app_user,"configure_catalog",&format!("package={id} codes={codes:?}")).await?;
    Ok(Redirect::to(&format!("/packages/{id}?notice=checklist#checklist")))
}
#[derive(Deserialize)]
struct CustomChecklistForm {
    csrf:String,title:String,#[serde(default)]note:String,
    #[serde(default)]required:Option<String>,#[serde(default)]remove:Option<String>,
}
fn checked_custom_title(value:&str)->WebResult<String>{
    let v=value.trim();
    if v.chars().count()<2 || v.chars().count()>300 || v.chars().any(char::is_control) {
        return Err(WebError::bad("Название позиции должно содержать от 2 до 300 символов"));
    }
    Ok(v.to_owned())
}
async fn create_custom_checklist(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<CustomChecklistForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    let _=repo::package(&st.db,id).await.map_err(|_|WebError::not_found())?;
    let title=checked_custom_title(&form.title)?;
    if form.note.chars().count()>500 {return Err(WebError::bad("Примечание длиннее 500 символов"));}
    let code=format!("USER-{}",uuid::Uuid::new_v4().simple());
    sqlx::query("INSERT INTO package_custom_checklist(package_id,code,title,required,note) VALUES($1,$2,$3,$4,$5)")
        .bind(id).bind(&code).bind(&title).bind(form.required.as_deref()==Some("yes"))
        .bind(&form.note).execute(&st.db).await?;
    engine::analyze(&st.db,&st.catalog,id,"checklist_custom_create").await?;
    repo::audit(&st.db,&st.cfg.app_user,"checklist_custom_create",&format!("package={id} code={code}")).await?;
    Ok(Redirect::to(&format!("/packages/{id}?notice=checklist#checklist")))
}
async fn edit_custom_checklist(State(st):State<Shared>,Path((id,item_id)):Path<(i64,i64)>,Form(form):Form<CustomChecklistForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    let _=repo::package(&st.db,id).await.map_err(|_|WebError::not_found())?;
    let title=checked_custom_title(&form.title)?;
    if form.note.chars().count()>500 {return Err(WebError::bad("Примечание длиннее 500 символов"));}
    let removal=form.remove.as_deref()==Some("yes");
    let row=sqlx::query("UPDATE package_custom_checklist SET title=$3,note=$4,required=$5,active=NOT $6,updated_at=now() WHERE package_id=$1 AND id=$2 AND active=true")
        .bind(id).bind(item_id).bind(&title).bind(&form.note)
        .bind(form.required.as_deref()==Some("yes")).bind(removal).execute(&st.db).await?;
    if row.rows_affected()!=1 {return Err(WebError::not_found());}
    if !removal {
        let code: (String,)=sqlx::query_as("SELECT code FROM package_custom_checklist WHERE package_id=$1 AND id=$2")
            .bind(id).bind(item_id).fetch_one(&st.db).await?;
        sqlx::query("UPDATE documents SET document_type=$3 WHERE package_id=$1 AND catalog_code=$2")
            .bind(id).bind(&code.0).bind(&title).execute(&st.db).await?;
    }
    // Existing uploaded copies and document assignments are intentionally preserved.
    engine::analyze(&st.db,&st.catalog,id,"checklist_custom_edit").await?;
    repo::audit(&st.db,&st.cfg.app_user,"checklist_custom_edit",&format!("package={id} item={item_id} removed={removal}")).await?;
    Ok(Redirect::to(&format!("/packages/{id}?notice=checklist#checklist")))
}
#[derive(sqlx::FromRow)]
struct AiSuggestionRow {
    id:i64, document_id:i64, reference_document_id:Option<i64>,
    source_quote:String, reference_quote:String, description:String,
    proposed_action:String, review_status:String, comment:String,
}
#[derive(Deserialize)]struct AiConsentForm { csrf:String, #[serde(default)]consent:Option<String> }
async fn set_ai_consent(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<AiConsentForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    let allow=form.consent.as_deref()==Some("yes");
    let affected=sqlx::query("UPDATE packages SET ai_consent=$1,ai_consent_updated_at=now() WHERE id=$2")
        .bind(allow).bind(id).execute(&st.db).await?;
    if affected.rows_affected()==0 {return Err(WebError::not_found());}
    if !allow {
        sqlx::query("UPDATE ai_jobs SET status='failed',error_message='Consent revoked',finished_at=now() WHERE package_id=$1 AND status='pending'")
            .bind(id).execute(&st.db).await?;
    }
    repo::audit(&st.db,&st.cfg.app_user,"ai_consent",&format!("package={id} allow={allow}")).await?;
    Ok(Redirect::to(&format!("/packages/{id}")))
}
async fn queue_ai_check(State(st):State<Shared>,Path(_id):Path<i64>,Form(form):Form<CsrfForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    Err(WebError::bad("Внешняя сверка ИД с ПД/РД отключена: запрещена передача проектной документации. Используйте локальные проверки."))
}
#[derive(Deserialize)]struct AiReviewForm{csrf:String,status:String,#[serde(default)]comment:String}
async fn review_ai_suggestion(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<AiReviewForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    if !["pending","confirmed","rejected"].contains(&form.status.as_str()) {return Err(WebError::bad("Invalid status"));}
    let row:Option<(i64,)> = sqlx::query_as("UPDATE ai_suggestions SET review_status=$1,comment=$2,reviewed_by=$3,reviewed_at=now() WHERE id=$4 RETURNING package_id")
        .bind(&form.status).bind(form.comment.chars().take(2000).collect::<String>())
        .bind(&st.cfg.app_user).bind(id).fetch_optional(&st.db).await?;
    let package_id=row.ok_or_else(WebError::not_found)?.0;
    repo::audit(&st.db,&st.cfg.app_user,"ai_suggestion_review",&format!("suggestion={id} status={}",form.status)).await?;
    Ok(Redirect::to(&format!("/packages/{package_id}")))
}
#[derive(Deserialize)]struct CsrfForm{csrf:String}
async fn manual_scan(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<CsrfForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    let _=repo::package(&st.db,id).await.map_err(|_|WebError::not_found())?;
    let _guard=st.scan_lock.lock().await;
    match scanner::scan_once(&st.db,&st.cfg,&st.catalog,&st.nas).await {
        Ok(_)=>st.synology_connected.store(true,Ordering::Relaxed),
        Err(e)=>{st.synology_connected.store(false,Ordering::Relaxed);return Err(e.into());}
    }
    engine::analyze(&st.db,&st.catalog,id,"manual").await?;
    repo::audit(&st.db,&st.cfg.app_user,"manual_scan",&format!("package={id}")).await?;
    Ok(Redirect::to(&format!("/packages/{id}")))
}
/// Upload up to 20 documents in one request. Files are staged locally first;
/// originals on NAS are never modified. A single checklist position may have many attachments.
async fn upload(State(st):State<Shared>,Path(id):Path<i64>,mut multipart:Multipart)->WebResult<Redirect>{
    let pkg=repo::package(&st.db,id).await.map_err(|_|WebError::not_found())?;
    let project=sqlx::query_as::<_,crate::models::Project>("SELECT id,name FROM projects WHERE id=$1")
        .bind(pkg.project_id).fetch_one(&st.db).await?;
    let mut kind=String::from("ID");let mut csrf=String::new();
    let mut catalog_code=String::new();
    let mut staged:Vec<(tempfile::NamedTempFile,String)>=Vec::new();
    let mut filenames=HashSet::new();
    while let Some(mut field)=multipart.next_field().await.map_err(|e|WebError::bad(e.to_string()))?{
        let field_name=field.name().unwrap_or("").to_owned();
        match field_name.as_str(){
            "csrf"=>csrf=field.text().await.map_err(|e|WebError::bad(e.to_string()))?,
            "kind"=>kind=field.text().await.map_err(|e|WebError::bad(e.to_string()))?,
            "catalog_code"=>catalog_code=field.text().await.map_err(|e|WebError::bad(e.to_string()))?,
            "file"=>{
                if staged.len()>=20 {return Err(WebError::bad("Можно загрузить не более 20 файлов за раз"));}
                let original_name=field.file_name().unwrap_or("file").to_owned();
                let filename=repo::validate_segment(&original_name).map_err(|_|WebError::bad("Некорректное имя файла"))?;
                if !crate::docs::supported(&PathBuf::from(filename)) || filename.starts_with("~$") {
                    return Err(WebError::bad("Неподдерживаемый формат файла"));
                }
                // Prevent file loss on Windows (case-insensitive names).
                if !filenames.insert(filename.to_lowercase()) {
                    return Err(WebError::bad("Одинаковые имена файлов в одной загрузке"));
                }
                let staging=st.cfg.temp_root.join("upload-staging");
                std::fs::create_dir_all(&staging)?;
                let mut tmp=tempfile::NamedTempFile::new_in(&staging)?;
                let mut count=0u64;
                while let Some(chunk)=field.chunk().await.map_err(|e|WebError::bad(e.to_string()))?{
                    count=count.saturating_add(chunk.len() as u64);
                    if count>st.cfg.max_file_mb*1024*1024 {return Err(WebError::bad("Файл превышает MAX_FILE_MB"));}
                    tmp.write_all(&chunk)?;
                }
                staged.push((tmp,filename.to_owned()));
            },
            _=>{}
        }
    }
    verify(&csrf,&st)?;
    if kind!="ID" && kind!="REFERENCE"{return Err(WebError::bad("Неизвестная категория документа"));}
    if staged.is_empty() {return Err(WebError::bad("Выбери файлы"));}
    if kind=="REFERENCE" && !catalog_code.is_empty(){return Err(WebError::bad("ПД/РД нельзя привязать к позиции ИД"));}
    let assigned=if catalog_code.is_empty(){None}else if let Some(item)=st.catalog.item(pkg.catalog_name.as_deref().unwrap_or(""),&catalog_code){
        Some((item.code.clone(),item.title.clone()))
    } else {
        let custom:Option<(String,String)>=sqlx::query_as("SELECT code,title FROM package_custom_checklist WHERE package_id=$1 AND code=$2 AND active=true")
            .bind(id).bind(&catalog_code).fetch_optional(&st.db).await?;
        Some(custom.ok_or_else(||WebError::bad("Позиция чек-листа не найдена"))?)
    };
    let _guard=st.scan_lock.lock().await;
    let root=st.cfg.data_root.join("inbox");
    let mut done=0usize;
    for (tmp,filename) in staged {
        let file=root.join(&project.name).join(&pkg.name).join(&kind).join(&filename);
        std::fs::create_dir_all(file.parent().unwrap())?;
        // Copy only into application inbox. Synology originals are read-only.
        std::fs::copy(tmp.path(),&file)?;
        scanner::ingest(&st.db,&st.cfg,&st.catalog,&root,&file,"upload").await?;
        if let Some((code,title))=&assigned {
            let rel=format!("{kind}/{filename}");
            if let Some(doc)=repo::latest_document(&st.db,id,&rel).await? {
                sqlx::query("UPDATE documents SET catalog_code=$1,document_type=$2 WHERE id=$3")
                    .bind(code).bind(title).bind(doc.id).execute(&st.db).await?;
            }
        }
        done+=1;
    }
    engine::analyze(&st.db,&st.catalog,id,"upload_multi").await?;
    repo::audit(&st.db,&st.cfg.app_user,"upload_multi",&format!("package={id} files={done} checklist={catalog_code}")).await?;
    Ok(Redirect::to(&format!("/packages/{id}?notice=uploaded#documents")))
}

#[derive(Deserialize)]struct ClassifyProviderForm {csrf:String,provider:String,#[serde(default)]auto_queue:Option<String>}
async fn set_classify_provider(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<ClassifyProviderForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    repo::package(&st.db,id).await.map_err(|_|WebError::not_found())?;
    if !["vibecode","gigacowork"].contains(&form.provider.as_str()) {return Err(WebError::bad("Unknown classification provider"));}
    sqlx::query("INSERT INTO classification_settings(package_id,provider,auto_queue) VALUES($1,$2,$3) ON CONFLICT(package_id) DO UPDATE SET provider=$2,auto_queue=$3,updated_at=now()")
      .bind(id).bind(&form.provider).bind(form.auto_queue.as_deref()==Some("yes")).execute(&st.db).await?;
    repo::audit(&st.db,&st.cfg.app_user,"classification_provider",&format!("package={id} provider={}",form.provider)).await?;
    Ok(Redirect::to(&format!("/packages/{id}")))
}
async fn allowed_classification(st:&Shared,id:i64)->WebResult<()> {
    repo::package(&st.db,id).await.map_err(|_|WebError::not_found())?;
    let consent:(bool,)=sqlx::query_as("SELECT ai_consent FROM packages WHERE id=$1").bind(id).fetch_one(&st.db).await?;
    if !consent.0 {return Err(WebError::bad("Set AI consent for this package first"));}
    let provider:Option<(String,)>=sqlx::query_as("SELECT provider FROM classification_settings WHERE package_id=$1")
       .bind(id).fetch_optional(&st.db).await?;
    let provider=provider.map(|p|p.0).unwrap_or_else(||"vibecode".into());
    if !classification::configured(&st.cfg,&provider){return Err(WebError::bad("Selected AI classification provider is not configured"));}
    Ok(())
}
async fn queue_classify_all(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<CsrfForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?; allowed_classification(&st,id).await?;
    let count=classification::enqueue(&st.db,id,false,&st.cfg.app_user).await?;
    repo::audit(&st.db,&st.cfg.app_user,"ai_classification_all",&format!("package={id} count={count}")).await?;
    Ok(Redirect::to(&format!("/packages/{id}")))
}
async fn queue_classify_unclassified(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<CsrfForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?; allowed_classification(&st,id).await?;
    let count=classification::enqueue(&st.db,id,true,&st.cfg.app_user).await?;
    repo::audit(&st.db,&st.cfg.app_user,"ai_classification_unclassified",&format!("package={id} count={count}")).await?;
    Ok(Redirect::to(&format!("/packages/{id}")))
}
async fn queue_classify_one(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<CsrfForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    let doc=repo::document(&st.db,id).await.map_err(|_|WebError::not_found())?;
    allowed_classification(&st,doc.package_id).await?;
    let ok=classification::enqueue_one(&st.db,id,&st.cfg.app_user).await?;
    if !ok {return Err(WebError::bad("Document has no extracted text, is obsolete, or is already queued"));}
    repo::audit(&st.db,&st.cfg.app_user,"ai_classification_one",&format!("doc={id}")).await?;
    Ok(Redirect::to(&format!("/packages/{}",doc.package_id)))
}
async fn confirm_classification(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<CsrfForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    let doc=repo::document(&st.db,id).await.map_err(|_|WebError::not_found())?;
    let row:Option<(String,String)>=sqlx::query_as("SELECT predicted_type,source_text_sha256 FROM classification_results WHERE document_id=$1 AND review_status='suggested'")
      .bind(id).fetch_optional(&st.db).await?;
    let (label,hash)=row.ok_or_else(||WebError::bad("No unreviewed AI classification"))?;
    use sha2::Digest;
    if label=="Не удалось определить" {
        return Err(WebError::bad("AI could not determine document type"));
    }
    if hex::encode(sha2::Sha256::digest(doc.recognized_text.as_bytes()))!=hash {
        return Err(WebError::bad("OCR text has changed; repeat classification"));
    }
    let pkg=repo::package(&st.db,doc.package_id).await?;
    let found=pkg.catalog_name.as_deref().and_then(|name|st.catalog.work_type(name)).and_then(|wt|{
       wt.items.iter().find(|item|crate::catalog::normalize(&item.title)==crate::catalog::normalize(&label))
    });
    let mut tx=st.db.begin().await?;
    sqlx::query("UPDATE documents SET document_type=$2,catalog_code=$3 WHERE id=$1")
       .bind(id).bind(&label).bind(found.map(|x|x.code.as_str())).execute(&mut *tx).await?;
    sqlx::query("UPDATE classification_results SET review_status='confirmed',reviewed_by=$2,reviewed_at=now() WHERE document_id=$1")
       .bind(id).bind(&st.cfg.app_user).execute(&mut *tx).await?;
    tx.commit().await?;
    engine::analyze(&st.db,&st.catalog,doc.package_id,"ai_classification_confirmed").await?;
    repo::audit(&st.db,&st.cfg.app_user,"ai_classification_confirmed",&format!("doc={id}")).await?;
    Ok(Redirect::to(&format!("/packages/{}",doc.package_id)))
}
async fn reject_classification(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<CsrfForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    let doc=repo::document(&st.db,id).await.map_err(|_|WebError::not_found())?;
    sqlx::query("UPDATE classification_results SET review_status='rejected',reviewed_by=$2,reviewed_at=now() WHERE document_id=$1 AND review_status='suggested'")
       .bind(id).bind(&st.cfg.app_user).execute(&st.db).await?;
    repo::audit(&st.db,&st.cfg.app_user,"ai_classification_rejected",&format!("doc={id}")).await?;
    Ok(Redirect::to(&format!("/packages/{}",doc.package_id)))
}
async fn classification_fragment(State(st):State<Shared>,Path(id):Path<i64>)->WebResult<Html<String>>{
    repo::package(&st.db,id).await.map_err(|_|WebError::not_found())?;
    let list:Vec<(i64,i64,String,String,String,String)>=sqlx::query_as(
        "SELECT id,document_id,provider,status,error_message,created_at::text FROM classification_jobs WHERE package_id=$1 ORDER BY id DESC LIMIT 25")
        .bind(id).fetch_all(&st.db).await?;
    let mut out=String::from("<table><tr><th>Задание</th><th>Документ</th><th>Провайдер</th><th>Статус</th><th>Ошибка</th></tr>");
    for (jid,did,provider,status,error,_date) in list {
        out.push_str(&format!("<tr><td>#{jid}</td><td>#{did}</td><td>{}</td><td>{}</td><td><small>{}</small></td></tr>",h(&provider),h(&status),h(&error)));
    }
    out.push_str("</table>");
    Ok(Html(out))
}

#[derive(Deserialize)]struct ClassifyForm{csrf:String,catalog_code:String}
async fn classify(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<ClassifyForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    let doc=repo::document(&st.db,id).await.map_err(|_|WebError::not_found())?;
    if doc.source_kind!="ID" {return Err(WebError::bad("ПД/РД нельзя назначать в позиции исполнительной документации"));}
    let pkg=repo::package(&st.db,doc.package_id).await?;
    let (code,title)=if form.catalog_code.is_empty(){(None,None)}else if let Some(item)=st.catalog.item(pkg.catalog_name.as_deref().unwrap_or(""),&form.catalog_code){
        (Some(item.code.clone()),Some(item.title.clone()))
    }else{
        let custom:Option<(String,String)>=sqlx::query_as("SELECT code,title FROM package_custom_checklist WHERE package_id=$1 AND code=$2 AND active=true")
            .bind(pkg.id).bind(&form.catalog_code).fetch_optional(&st.db).await?;
        let (c,t)=custom.ok_or_else(||WebError::bad("Неизвестная позиция чек-листа"))?;
        (Some(c),Some(t))
    };
    sqlx::query("UPDATE documents SET catalog_code=$1,document_type=$2 WHERE id=$3")
        .bind(&code).bind(&title).bind(id).execute(&st.db).await?;
    sqlx::query("UPDATE classification_results SET review_status='manual_override',reviewed_by=$2,reviewed_at=now() WHERE document_id=$1")
        .bind(id).bind(&st.cfg.app_user).execute(&st.db).await?;
    engine::analyze(&st.db,&st.catalog,pkg.id,"manual").await?;
    repo::audit(&st.db,&st.cfg.app_user,"classify_document",&format!("doc={id} code={}",form.catalog_code)).await?;
    Ok(Redirect::to(&format!("/packages/{}?notice=classified&doc={id}#doc-{id}",pkg.id)))
}
#[derive(Deserialize)]struct ReviewForm{csrf:String,status:String,#[serde(default)]comment:String}
async fn review(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<ReviewForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    if !["pending","confirmed","rejected","resolved"].contains(&form.status.as_str()){
        return Err(WebError::bad("Invalid review status"));
    }
    let row:Option<(i64,)>=sqlx::query_as("UPDATE findings SET review_status=$1,comment=$2,reviewed_by=$3,reviewed_at=now() WHERE id=$4 RETURNING package_id")
        .bind(&form.status).bind(form.comment.chars().take(5000).collect::<String>()).bind(&st.cfg.app_user).bind(id)
        .fetch_optional(&st.db).await?;
    let package_id=row.ok_or_else(WebError::not_found)?.0;
    repo::audit(&st.db,&st.cfg.app_user,"review",&format!("finding={id} status={}",form.status)).await?;
    Ok(Redirect::to(&format!("/packages/{package_id}")))
}
async fn download(State(st):State<Shared>,Path(id):Path<i64>)->WebResult<Response>{
    let doc=repo::document(&st.db,id).await.map_err(|_|WebError::not_found())?;
    let path=&doc.archive_path;
    let safe=std::fs::canonicalize(path).map_err(|_|WebError::not_found())?;
    let root=std::fs::canonicalize(&st.cfg.data_root)?;
    if !safe.starts_with(&root){return Err(WebError{status:StatusCode::FORBIDDEN,detail:"Invalid storage path".into()});}
    let file=tokio::fs::File::open(safe).await?;
    let stream=tokio_util::io::ReaderStream::new(file);
    let fname=format!("document-{}-v{}.{}",id,doc.version,doc.relative_path.rsplit('.').next().unwrap_or("bin"));
    let response=Response::builder().header(header::CONTENT_TYPE,"application/octet-stream")
        .header(header::CONTENT_DISPOSITION,format!("attachment; filename=\"{}\"",fname))
        .body(Body::from_stream(stream))?;
    Ok(response)
}
#[derive(Deserialize,Default)]struct ReportQuery{#[serde(default)]draft:bool,run_id:Option<i64>}
async fn report(State(st):State<Shared>,Path((id,fmt)):Path<(i64,String)>,Query(query):Query<ReportQuery>)->WebResult<Response>{
    if !["xlsx","docx","pdf"].contains(&fmt.as_str()){return Err(WebError::bad("Unknown report format"));}
    let _=repo::package(&st.db,id).await.map_err(|_|WebError::not_found())?;
    let runs=repo::runs(&st.db,id).await?;
    let run=if let Some(want)=query.run_id {runs.iter().find(|r|r.id==want)}else{runs.first()};
    let run=run.ok_or_else(WebError::not_found)?;
    let all=repo::findings(&st.db,run.id).await?;
    let filtered:Vec<Finding>=all.into_iter().filter(|f|query.draft || ["confirmed","resolved"].contains(&f.review_status.as_str())).collect();
    let fmt_for_task=fmt.clone();
    let temp_root=st.cfg.temp_root.clone();
    let output=tokio::task::spawn_blocking(move||->Result<Vec<u8>>{
        match fmt_for_task.as_str(){"xlsx"=>reports::xlsx(&filtered),"docx"=>reports::docx(&filtered),"pdf"=>reports::pdf(&filtered,&temp_root),_=>unreachable!()}
    }).await??;
    let mime=match fmt.as_str(){"xlsx"=>"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet","docx"=>"application/vnd.openxmlformats-officedocument.wordprocessingml.document","pdf"=>"application/pdf",_=>"application/octet-stream"};
    Ok(Response::builder().header(header::CONTENT_TYPE,mime)
        .header(header::CONTENT_DISPOSITION,format!("attachment; filename=\"findings-{}-{}.{}\"",id,run.id,fmt))
        .body(Body::from(output))?)
}

#[derive(Deserialize,Default)]
struct OcrJobQuery {package_id:Option<i64>}

fn status_ru(status:&str)->&'static str {
    match status {
        "pending"=>"В очереди",
        "running"=>"Выполняется",
        "done"=>"Готово",
        "partial"=>"Частично / ручная проверка",
        "failed"=>"Ошибка",
        "cancelled"=>"Отменено",
        _=>"Неизвестно"
    }
}
async fn jobs_markup(st:&Shared,package_id:Option<i64>)->WebResult<String>{
    let jobs=ocr_jobs::jobs(&st.db,package_id,15).await?;
    if jobs.is_empty(){return Ok("<p class=muted>Заданий пока нет. Выбери документ и нажми «Повторить OCR».</p>".into());}
    let mut html=String::new();
    for job in jobs {
        let filename:Option<(String,)>=sqlx::query_as("SELECT relative_path FROM documents WHERE id=$1")
            .bind(job.document_id).fetch_optional(&st.db).await?;
        let name=filename.map(|x|x.0).unwrap_or_else(||format!("Документ #{}",job.document_id));
        let class=match job.status.as_str(){"failed"=>"job-failed","done"=>"job-done","partial"=>"job-partial","running"=>"job-running",_=>""};
        html.push_str(&format!("<div class='job {class}' data-document-id='{}' data-job-status='{}'><div class=row><b>#{} — {}</b><span class=chip>{}</span><small>Обновлено: {}</small></div><p>{}</p>",
            job.document_id,h(&job.status),job.id,h(&name),status_ru(&job.status),job.updated_at.format("%d.%m %H:%M:%S UTC"),h(&job.message)));
        if job.total_steps>0 {
            html.push_str(&format!("<p><progress value='{}' max='{}'></progress> {} / {} · этап {}</p>",job.current_step.clamp(0,job.total_steps),job.total_steps,job.current_step,job.total_steps,h(&job.stage)));
        }else {html.push_str(&format!("<p><small>Этап: {}</small></p>",h(&job.stage)));}
        if !job.error_message.is_empty(){html.push_str(&format!("<p class=warn>Ошибка: {}</p>",h(&job.error_message)));}
        if job.status=="pending" {
            html.push_str(&format!("<form method=post action='/ocr/jobs/{}/cancel'>{}<button class=warnbutton>Отменить ожидание</button></form>",job.id,hidden(&st.csrf)));
        }
        let events=ocr_jobs::recent_events(&st.db,job.id,15).await?;
        if !events.is_empty(){
            html.push_str(&format!("<details data-job='{}'><summary>Этапы и сообщения</summary><table>",job.id));
            html.push_str("<tr><th>Время (UTC)</th><th>Этап</th><th>Прогресс</th><th>Сообщение</th></tr>");
            for event in events {
                html.push_str(&format!("<tr><td>{}</td><td>{}</td><td>{}/{}</td><td>{}</td></tr>",
                    event.created_at.format("%H:%M:%S"),h(&event.stage),event.current_step,event.total_steps,h(&event.message)));
            }
            html.push_str("</table></details>");
        }
        html.push_str("</div>");
    }
    Ok(html)
}

async fn ocr_jobs_fragment(State(st):State<Shared>,Query(q):Query<OcrJobQuery>)->WebResult<Html<String>>{
    Ok(Html(jobs_markup(&st,q.package_id).await?))
}
async fn ocr_jobs_json(State(st):State<Shared>,Query(q):Query<OcrJobQuery>)->WebResult<Json<Vec<ocr_jobs::OcrJob>>>{
    Ok(Json(ocr_jobs::jobs(&st.db,q.package_id,100).await?))
}
async fn ocr_status_json(State(st):State<Shared>)->WebResult<Json<serde_json::Value>>{
    let counts:Vec<(String,i64)>=sqlx::query_as("SELECT status,count(*) FROM ocr_jobs GROUP BY status ORDER BY status")
        .fetch_all(&st.db).await?;
    let scanning=st.scan_status.read().map(|state|state.running).unwrap_or(false);
    let scan_message=st.scan_status.read().map(|state|state.message.clone()).unwrap_or_default();
    Ok(Json(serde_json::json!({
        "app_version":"0.9.0","ocr_mode":st.cfg.ocr_mode,
        "data_root":st.cfg.data_root.to_string_lossy(),
        "temp_root":st.cfg.temp_root.to_string_lossy(),
        "local_ocr_model_dir":st.cfg.local_ocr_model_dir.to_string_lossy(),
        "local_models_ready":crate::local_ocr::ensure_models(&st.cfg.local_ocr_model_dir).is_ok(),
        "vibe_ocr_enabled":st.cfg.vibe_ocr_enabled,
        "vibe_ocr_key_configured":st.cfg.vibe_ocr_key.is_some(),
        "vibe_timeout_seconds":st.cfg.vibe_ocr_timeout,
        "ocr_max_pages":st.cfg.ocr_max_pages,"ocr_page_workers":st.cfg.ocr_page_workers,"job_counts":counts,
        "synology_connected":st.synology_connected.load(Ordering::Relaxed),
        "scan_running":scanning,"scan_message":scan_message
    })))
}

async fn diagnostics(State(st):State<Shared>)->WebResult<Html<String>>{
    let stats:Vec<(String,i64)>=sqlx::query_as(
        "SELECT CASE WHEN recognition_status LIKE 'error:%' THEN 'error' ELSE recognition_status END,
         count(*) FROM documents d WHERE d.is_present=true
         AND NOT EXISTS(SELECT 1 FROM documents newer WHERE newer.package_id=d.package_id
         AND newer.relative_path=d.relative_path AND newer.version>d.version)
         GROUP BY 1 ORDER BY 1").fetch_all(&st.db).await?;
    let queue_counts:Vec<(String,i64)>=sqlx::query_as("SELECT status,count(*) FROM ocr_jobs GROUP BY status ORDER BY status")
        .fetch_all(&st.db).await?;
    let last_errors:Vec<(i64,String,String)>=sqlx::query_as(
        "SELECT id,relative_path,recognition_status FROM documents WHERE is_present=true
         AND recognition_status LIKE 'error:%' ORDER BY id DESC LIMIT 15")
        .fetch_all(&st.db).await?;
    let scan=st.scan_status.read().map(|s|s.clone()).unwrap_or_default();
    let mut body=String::from("<h1>Диагностика системы и OCR</h1><p>Здесь отображаются фактические настройки запуска, очередь и события. API-ключ и содержимое документов в журнал не записываются.</p>");
    body.push_str("<article><h2>Компоненты</h2><table><tr><th>Компонент</th><th>Состояние</th></tr>");
    body.push_str(&format!("<tr><td>Приложение</td><td>v0.9.0 · порт {}</td></tr>",h(&st.cfg.listen)));
    body.push_str(&format!("<tr><td>Локальный архив DATA_ROOT</td><td>{}</td></tr><tr><td>Временные файлы TEMP_ROOT</td><td>{}</td></tr>",
        h(&st.cfg.data_root.to_string_lossy()),h(&st.cfg.temp_root.to_string_lossy())));
    body.push_str(&format!("<tr><td>PostgreSQL</td><td>Подключена (данные получены)</td></tr><tr><td>Synology</td><td>{}</td></tr>",
        if st.synology_connected.load(Ordering::Relaxed){"Связь при последней проверке была успешна"}else{"Нет подтверждения успешной связи"}));
    body.push_str(&format!("<tr><td>Сканирование</td><td>{} · {} · последний запуск {}</td></tr>",
        if scan.running{"Выполняется"}else{"Не выполняется"},h(&scan.message),
        scan.last_at.map(|t|t.format("%Y-%m-%d %H:%M:%S UTC").to_string()).unwrap_or_else(||"нет данных".into())));
    let model_check=crate::local_ocr::ensure_models(&st.cfg.local_ocr_model_dir);
    body.push_str(&format!("<tr><td>OCR режим</td><td><b>{}</b> · локальные ONNX-модели: {} · путь {}</td></tr>",
        h(&st.cfg.ocr_mode),if model_check.is_ok(){"НАЙДЕНЫ"}else{"НЕ УСТАНОВЛЕНЫ"},h(&st.cfg.local_ocr_model_dir.to_string_lossy())));
    if let Err(error)=model_check {body.push_str(&format!("<tr><td>Проверка моделей</td><td>{}</td></tr>",h(&error.to_string())));}
    body.push_str(&format!("<tr><td>Параллельный OCR</td><td>Максимум {} ONNX-потоков одновременно, до {} страниц PDF, ограниченная очередь страниц (2 на поток). Для одного PDF используется несколько потоков; для одностраничного файла — один.</td></tr>",
        st.cfg.ocr_page_workers,st.cfg.ocr_max_pages));

    body.push_str(&format!("<tr><td>VibeCode OCR</td><td>enabled={} · key_present={} · HTTP timeout={} сек./попытку · PDF max={} стр.</td></tr>",
        st.cfg.vibe_ocr_enabled,st.cfg.vibe_ocr_key.is_some(),st.cfg.vibe_ocr_timeout,st.cfg.ocr_max_pages));
    let class_counts:Vec<(String,i64)>=sqlx::query_as(
        "SELECT status,count(*) FROM classification_jobs GROUP BY status ORDER BY status")
        .fetch_all(&st.db).await?;
    let classifier_status=class_counts.iter().map(|(status,n)|format!("{}: {}",status,n)).collect::<Vec<_>>().join(", ");
    body.push_str(&format!("<tr><td>ИИ-классификатор</td><td>VibeCode: {} · GigaCowork-адаптер: {} · очередь: {}</td></tr>",
        if classification::configured(&st.cfg,"vibecode") {"настроен"} else {"не настроен"},
        if classification::configured(&st.cfg,"gigacowork") {"настроен"} else {"не настроен"},
        h(&classifier_status)));
    body.push_str(&format!("<tr><td>Synology TLS</td><td>{}</td></tr>",if st.cfg.synology_verify_tls{"Проверка сертификата включена"}else{"ВНИМАНИЕ: проверка сертификата отключена"}));
    body.push_str("</table></article><article><h2>Документы: результат извлечения</h2><table><tr><th>Статус</th><th>Количество</th></tr>");
    for (status,count) in stats {body.push_str(&format!("<tr><td>{}</td><td>{count}</td></tr>",h(&status)));}
    body.push_str("</table><h3>Последние ошибки</h3><table><tr><th>Документ</th><th>Ошибка</th><th>Действие</th></tr>");
    for (id,name,status) in last_errors {
        body.push_str(&format!("<tr><td>{}</td><td>{}</td><td><form method=post action='/documents/{id}/reprocess'>{}<button>Повторить OCR</button></form></td></tr>",h(&name),h(&status),hidden(&st.csrf)));
    }
    body.push_str("</table></article><article><h2>Очередь OCR</h2><p>");
    for (state,count) in queue_counts {body.push_str(&format!("<span class=chip>{}: {count}</span> ",status_ru(&state)));}
    body.push_str("</p><small id=ocr-poll-state></small><div class='ocr-live' data-url='/ocr/jobs/fragment'>Загрузка журнала...</div></article>");
    body.push_str("<article><h2>Команды диагностики</h2><pre>cargo run -- --ocr-status\ncargo run -- --check-vibe-ocr tests\\fixtures\\sample.tif\n$env:RUST_LOG='id_control_axum=debug,info'\ncargo run</pre><p>При работающем сервере: <a href='/api/ocr/status'>JSON состояния</a> · <a href='/api/ocr/jobs'>JSON заданий</a>. Пароли и ключи в отчёт не включаются.</p></article>");
    Ok(layout("Диагностика",body))
}

async fn reprocess_document(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<CsrfForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    let doc=repo::document(&st.db,id).await.map_err(|_|WebError::not_found())?;
    let queued=ocr_jobs::enqueue_document(&st.db,id,&st.cfg.app_user).await.map_err(|e|WebError::bad(e.to_string()))?;
    if let Some(job)=queued {repo::audit(&st.db,&st.cfg.app_user,"ocr_single_queued",&format!("doc={id} job={job}")).await?;}
    Ok(Redirect::to(&format!("/packages/{}",doc.package_id)))
}
async fn reprocess_package_errors(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<CsrfForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    repo::package(&st.db,id).await.map_err(|_|WebError::not_found())?;
    let n=ocr_jobs::enqueue_package(&st.db,id,false,&st.cfg.app_user).await?;
    repo::audit(&st.db,&st.cfg.app_user,"ocr_package_errors_queued",&format!("package={id} jobs={n}")).await?;
    Ok(Redirect::to(&format!("/packages/{id}")))
}
async fn reprocess_package_all(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<CsrfForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    repo::package(&st.db,id).await.map_err(|_|WebError::not_found())?;
    let n=ocr_jobs::enqueue_package(&st.db,id,true,&st.cfg.app_user).await?;
    repo::audit(&st.db,&st.cfg.app_user,"ocr_package_all_queued",&format!("package={id} jobs={n}")).await?;
    Ok(Redirect::to(&format!("/packages/{id}")))
}
async fn cancel_ocr_job(State(st):State<Shared>,Path(id):Path<i64>,Form(form):Form<CsrfForm>)->WebResult<Redirect>{
    verify(&form.csrf,&st)?;
    let pkg:Option<(i64,)>=sqlx::query_as("SELECT package_id FROM ocr_jobs WHERE id=$1").bind(id).fetch_optional(&st.db).await?;
    let pkg=pkg.ok_or_else(WebError::not_found)?.0;
    let _=ocr_jobs::cancel_pending(&st.db,id).await?;
    Ok(Redirect::to(&format!("/packages/{pkg}")))
}

async fn health(State(st):State<Shared>)->WebResult<Json<serde_json::Value>>{
    let approved:(i64,) = sqlx::query_as("SELECT count(*) FROM normative_rules WHERE approval_status='approved'")
        .fetch_one(&st.db).await?;
    Ok(Json(serde_json::json!({"synology_connected":st.synology_connected.load(Ordering::Relaxed),
        "approved_normative_rules":approved.0,"normative_rules_configured":approved.0>0,
        "app_version":"0.9.0"})))
}
