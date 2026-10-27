//! Stage 2.3. Independent, one-use contractor links and safe DSM uploads.
//! No Bitrix cookie is required on public /contractor/:token routes.
use crate::{access::{self,Principal},engine,portal,repo,synology::{SynologyClient,validate_resdata_path},web::{self,Shared,WebError}};
use anyhow::{bail,Context,Result};
use axum::{extract::{State,Path,Query,Multipart},Extension,Form,Json,
    response::{Html,IntoResponse,Response},http::{header,StatusCode}};
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json,Value};
use tokio::io::AsyncWriteExt;
use std::sync::{Arc,atomic::{AtomicU64,AtomicU8,Ordering}};
use sha2::{Digest,Sha256};
use sqlx::Row;
use std::{fs::{self,File},io::{Read,Write},path::{Path as FsPath,PathBuf}};
use zip::ZipArchive;

fn h(s:&str)->String{html_escape::encode_safe(s).to_string()}
/// The catalogue includes work names containing '/': never use these verbatim
/// as NAS path components. A stable human-readable folder name is sufficient.
pub fn nas_work_folder(name:&str)->String{name.replace('/'," - ")}

fn digest(data:&[u8])->String{hex::encode(Sha256::digest(data))}
fn actor_name(actor:&Principal)->String{format!("bitrix:{}:{}",actor.member,actor.user_id)}
fn valid_token(s:&str)->bool{s.len()==64 && s.as_bytes().iter().all(|b|b.is_ascii_hexdigit())}
fn public_page(content:String)->Response {
    let upload_js=include_str!("../assets/contractor-upload.js");
    let html=format!(r#"<!doctype html><html lang="ru"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><meta name="referrer" content="no-referrer"><title>Загрузка комплекта ИД</title><style>body{{margin:0;font:16px system-ui;background:#eff5fa;color:#172c46}}main{{max-width:740px;margin:6vh auto;background:#fff;padding:32px;border-radius:16px;box-shadow:0 8px 32px #223b5a1a}}h1{{margin-top:0}}label{{display:block;margin:16px 0}}input,button{{padding:12px;font:inherit;border-radius:8px}}input[type=file]{{max-width:100%;border:1px solid #cdd9e4}}progress{{accent-color:#0871e8;height:14px}}button{{background:#006fee;color:#fff;border:0;cursor:pointer}}small{{color:#52697f}}@media(max-width:700px){{main{{margin:12px;padding:20px}}}}</style></head><body><main>{content}</main><script>{upload_js}</script></body></html>"#);
    let mut response=Html(html).into_response();
    response.headers_mut().insert(header::CACHE_CONTROL,header::HeaderValue::from_static("no-store"));
    response.headers_mut().insert(header::HeaderName::from_static("referrer-policy"),header::HeaderValue::from_static("no-referrer"));
    response.headers_mut().insert(header::X_CONTENT_TYPE_OPTIONS,header::HeaderValue::from_static("nosniff"));
    response
}

/// Saved in PostgreSQL; if the administrator has not selected a folder yet,
/// honor the existing .env configuration. This is *not* an arbitrary user path.
pub async fn selected_root(st:&Shared)->Result<String>{
    selected_root_db(&st.db,&st.cfg.synology_root).await
}
pub async fn selected_root_db(db:&sqlx::PgPool,fallback:&str)->Result<String>{
    Ok(sqlx::query_scalar::<_,String>("SELECT setting_value FROM app_settings WHERE setting_key='synology_root'")
        .fetch_optional(db).await?.unwrap_or_else(||fallback.to_owned()))
}

#[derive(Deserialize)]pub struct Browse {folder:Option<String>,saved:Option<String>}
pub async fn storage_view(State(st):State<Shared>,Extension(actor):Extension<Principal>,Query(params):Query<Browse>)->std::result::Result<Html<String>,WebError>{
    if !actor.superuser {return Err(WebError::forbidden());}
    let current=selected_root(&st).await?;
    let folder=params.folder.unwrap_or_else(||"/ResData".to_string());
    validate_resdata_path(&folder).map_err(|_|WebError::bad("Разрешены только папки внутри /ResData"))?;
    let session=st.nas.connect().await.map_err(|err|WebError::bad(format!("Нет соединения с Synology: {err:#}")))?;
    let dirs=session.list_directories(&folder).await;
    session.logout().await;
    let dirs=dirs.map_err(|err|WebError::bad(format!("Не удалось прочитать каталог: {err:#}")))?;
    let mut body=format!("<h1>Папка проектов на Synology</h1><p>Корневая папка выбирается один раз для всех шести ЖК. Доступны только каталоги внутри <code>/ResData</code>. Имеющиеся данные и папки не удаляются.</p><article><p><b>Текущая папка:</b> <code>{}</code></p><p><b>Просматриваем:</b> <code>{}</code></p>",h(&current),h(&folder));
    if params.saved.as_deref()==Some("1") {body.push_str("<p class='notice'>Корневая папка сохранена. Новый выбор применяется к следующим сканированиям и загрузкам.</p>");}
    body.push_str(&format!("<form method='post' action='/administration/storage'>{}<input type='hidden' name='root' value='{}'><button type='submit'>Использовать эту папку для проектов</button></form>",web::hidden(&st.csrf),h(&folder)));
    body.push_str("<h2>Вложенные папки</h2><ul>");
    if folder!="/ResData" {
        let parent=folder.rsplit_once('/').map(|(p,_)|p).unwrap_or("/ResData");
        body.push_str(&format!("<li><a href='/administration/storage?folder={}'>← На уровень выше</a></li>",url::form_urlencoded::byte_serialize(parent.as_bytes()).collect::<String>()));
    }
    for child in dirs{
        let query=url::form_urlencoded::byte_serialize(child.as_bytes()).collect::<String>();
        body.push_str(&format!("<li><a href='/administration/storage?folder={query}'>📁 {}</a></li>",h(child.rsplit('/').next().unwrap_or(&child))));
    }
    body.push_str("</ul><p><a href='/administration'>← Администрирование</a></p></article>");
    Ok(web::layout("Папки Synology",body))
}
#[derive(Deserialize)]pub struct RootForm{csrf:String,root:String}
pub async fn storage_save(State(st):State<Shared>,Extension(actor):Extension<Principal>,Form(form):Form<RootForm>)->std::result::Result<axum::response::Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    if !actor.superuser{return Err(WebError::forbidden());}
    validate_resdata_path(&form.root).map_err(|_|WebError::bad("Папка должна быть внутри /ResData"))?;
    // Read the folder itself to verify it exists and DSM grants read access.
    let session=st.nas.connect().await.map_err(|e|WebError::bad(format!("DSM: {e:#}")))?;
    let checked=session.list_directories(&form.root).await;
    session.logout().await;
    checked.map_err(|e|WebError::bad(format!("DSM не подтверждает доступ к папке: {e:#}")))?;
    sqlx::query("INSERT INTO app_settings(setting_key,setting_value) VALUES('synology_root',$1) ON CONFLICT(setting_key) DO UPDATE SET setting_value=$1,updated_at=now()")
        .bind(&form.root).execute(&st.db).await?;
    repo::audit(&st.db,&actor_name(&actor),"synology_root_changed",&form.root).await?;
    Ok(axum::response::Redirect::to("/administration/storage?saved=1"))
}

#[derive(Deserialize)]pub struct NewLinkForm{csrf:String}
pub async fn create_link(State(st):State<Shared>,Extension(actor):Extension<Principal>,Path(package_id):Path<i64>,Form(form):Form<NewLinkForm>)->std::result::Result<Html<String>,WebError>{
    web::verify(&form.csrf,&st)?;
    let role=access::package_role(&st,&actor,package_id).await.ok_or_else(WebError::forbidden)?;
    if role!="manager" && role!="engineer" {return Err(WebError::forbidden());}
    let pkg=repo::package(&st.db,package_id).await.map_err(|_|WebError::not_found())?;
    let project=sqlx::query_scalar::<_,String>("SELECT name FROM projects WHERE id=$1")
        .bind(pkg.project_id).fetch_optional(&st.db).await?.ok_or_else(WebError::not_found)?;
    if portal::project_info(&project).is_none() {return Err(WebError::not_found());}
    let root=selected_root(&st).await?;
    validate_resdata_path(&root).map_err(|_|WebError::bad("Суперадминистратор должен сначала выбрать корневую папку /ResData"))?;
    let mut raw=[0u8;32];rand::thread_rng().fill_bytes(&mut raw);
    let token=hex::encode(raw);
    sqlx::query("INSERT INTO contractor_upload_links(token_hash,package_id,created_by,expires_at,nas_root) VALUES($1,$2,$3,now()+interval '72 hours',$4)")
        .bind(digest(token.as_bytes())).bind(package_id).bind(actor_name(&actor)).bind(root).execute(&st.db).await?;
    repo::audit(&st.db,&actor_name(&actor),"contractor_link_created",&format!("package={package_id}")).await?;
    let path=format!("/contractor/{token}");
    let body=format!(r#"<h1>Одноразовая ссылка для подрядчика</h1><p><b>{}</b> — {}</p><p>Действует 72 часа и принимает ровно один ZIP-архив. При повторной загрузке понадобится новая ссылка.</p><article><label>Ссылка для передачи подрядчику <input id='contractor-url' readonly style='width:95%'></label><button type='button' onclick="navigator.clipboard.writeText(document.getElementById('contractor-url').value)">Скопировать</button><script>document.getElementById('contractor-url').value=new URL('{}',location.href).href;</script></article><p><a href='/packages/{}#contractor'>Вернуться к комплекту</a></p>"#,h(&project),h(&pkg.name),path,package_id);
    Ok(web::layout("Ссылка подрядчику",body))
}

// Link metadata is bound to package_id server-side. No employee identity, project,
// work type or destination can be supplied by the public requester.
async fn link_details(st:&Shared,token:&str)->std::result::Result<Option<(i64,String,String)>,WebError>{
    if !valid_token(token){return Ok(None);}
    let row=sqlx::query("SELECT l.package_id,p.name AS project_name,k.name AS work_name FROM contractor_upload_links l JOIN packages k ON k.id=l.package_id JOIN projects p ON p.id=k.project_id WHERE l.token_hash=$1 AND l.claimed_at IS NULL AND l.revoked_at IS NULL AND l.expires_at>now()")
        .bind(digest(token.as_bytes())).fetch_optional(&st.db).await?;
    if let Some(row)=row {Ok(Some((row.try_get("package_id")?,row.try_get("project_name")?,row.try_get("work_name")?)))}else{Ok(None)}
}
fn progress_panel(token:&str)->String {
    format!(r#"<section id="delivery-progress" data-status-url="/contractor/{token}/status" hidden aria-live="polite">
        <h2>Обработка комплекта</h2><p id="processing-stage">Подготовка…</p>
        <progress id="processing-bar" max="100" value="0" style="width:100%"></progress>
        <p id="processing-value">0 %</p><p id="processing-detail"></p>
    </section>"#)
}

pub async fn public_form(State(st):State<Shared>,Path(token):Path<String>)->Response {
    if !valid_token(&token){return public_page("<h1>Ссылка недоступна</h1>".into());}
    // A claimed link remains a read-only status/receipt page. Reloading it
    // must not lose track of a job already running on the server.
    if let Ok(Some(_))=submission_status(&st,&token).await {
        return public_page(format!("<h1>Приём комплекта ИД</h1><p>Ссылка использована для одной поставки. Здесь можно отслеживать её обработку.</p>{}",progress_panel(&token)));
    }
    let found=link_details(&st,&token).await.ok().flatten();
    match found {
        Some((_,project,work))=>public_page(format!(r#"<h1>Передача исполнительной документации</h1><p><b>ЖК: {}</b></p><p>Вид работ: <b>{}</b></p><p>Загрузите один ZIP-архив. Ссылка одноразовая. После приёма ZIP дальнейшая обработка идёт в фоне, даже если закрыть страницу.</p><form action='/contractor/{}' method='post' enctype='multipart/form-data'><label>ZIP-архив <input type='file' name='archive' accept='.zip' required></label><button type='submit'>Передать комплект</button><p id='send-status' role='status' aria-live='polite'></p><progress id='send-bar' value='0' max='100' style='width:100%' hidden></progress></form>{}<p><small>В архиве не допускаются исполняемые файлы и ссылки. После отправки дождитесь регистрации поставки, а затем можно закрыть страницу.</small></p>"#,h(&project),h(&work),h(&token),progress_panel(&token))),
        None=>public_page("<h1>Ссылка недоступна</h1><p>Срок ссылки истёк либо доступ отозван. Запросите новую ссылку у ответственного сотрудника.</p>".into()),
    }
}

/// Unauthenticated status access is scoped to the unguessable original 256-bit link token.
/// The token cannot reveal other projects, employees, or packages.
async fn submission_status(st:&Shared,token:&str)->Result<Option<Value>> {
    if !valid_token(token){return Ok(None);}
    let row=sqlx::query("SELECT s.package_id,s.revision,s.status,s.processing_stage,s.processing_percent,s.processed_bytes,s.processing_total_bytes,s.error_message FROM contractor_upload_links l JOIN contractor_submissions s ON s.link_id=l.id WHERE l.token_hash=$1")
        .bind(digest(token.as_bytes())).fetch_optional(&st.db).await?;
    let Some(row)=row else {return Ok(None)};
    let package_id:i64=row.try_get("package_id")?;
    let revision:i32=row.try_get("revision")?;
    let status:String=row.try_get("status")?;
    let stage:String=row.try_get("processing_stage")?;
    let done=matches!(status.as_str(),"pending_review"|"checked"|"rework");
    let failed=status=="failed";
    // Only generic failures are exposed to contractors; details stay in admin UI/logs.
    Ok(Some(json!({
        "receipt":format!("ИД-{package_id}-{revision:05}"),
        "status":status,"stage":stage,
        "percent":row.try_get::<i16,_>("processing_percent")?,
        "bytes":row.try_get::<i64,_>("processed_bytes")?,
        "total_bytes":row.try_get::<i64,_>("processing_total_bytes")?,
        "complete":done,"failed":failed,
        "message":if failed {"Обработка не завершена. Обратитесь к ответственному сотруднику для новой ссылки."} else {""}
    })))
}

pub async fn public_status(State(st):State<Shared>,Path(token):Path<String>)->Response {
    let result=submission_status(&st,&token).await;
    let mut response=match result {
        Ok(Some(value))=>(StatusCode::OK,Json(value)).into_response(),
        Ok(None)=>(StatusCode::NOT_FOUND,Json(json!({"error":"Поставка пока не зарегистрирована"}))).into_response(),
        Err(err)=>{
            tracing::error!("Contractor progress read error: {err:#}");
            (StatusCode::SERVICE_UNAVAILABLE,Json(json!({"error":"Временно недоступен статус"}))).into_response()
        }
    };
    response.headers_mut().insert(header::CACHE_CONTROL,header::HeaderValue::from_static("no-store"));
    response
}

struct SafeFile {relative:String,local:PathBuf,sha256:String,size:u64}
fn unpack(zip_path:&FsPath,output:&FsPath,per_file_limit:u64,on_progress:&dyn Fn(u64,u64))->Result<Vec<SafeFile>>{
    let file=File::open(zip_path)?;
    let mut zip=ZipArchive::new(file).context("Невозможно прочитать ZIP")?;
    if zip.len()>1000 {bail!("В архиве больше 1000 элементов");}
    let mut expected=0u64;
    for idx in 0..zip.len() {
        let entry=zip.by_index(idx)?;
        if !entry.is_dir(){expected=expected.saturating_add(entry.size());}
    }
    let mut extracted=0u64;
    let mut files=Vec::new();
    let mut total=0u64;
    let mut names=std::collections::HashSet::new();
    for idx in 0..zip.len(){
        let mut entry=zip.by_index(idx)?;
        if entry.is_dir(){continue;}
        if entry.unix_mode().is_some_and(|m|m & 0o170000 == 0o120000){bail!("Символические ссылки в архиве запрещены");}
        let enclosed=entry.enclosed_name().context("В ZIP обнаружен небезопасный путь")?.to_owned();
        let segments=enclosed.components().map(|c|c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>();
        if segments.is_empty() || segments.len()>12 {bail!("Некорректная глубина вложенности ZIP");}
        if segments.iter().any(|s|repo::validate_segment(s).is_err() || s.contains(':')){bail!("Некорректное имя в ZIP");}
        let relative=segments.join("/");
        if !names.insert(relative.to_lowercase()){bail!("Повторяющиеся имена файлов в архиве");}
        if !crate::docs::supported(&enclosed) {bail!("Неподдерживаемый файл в ZIP: {relative}");}
        if entry.size()>per_file_limit {bail!("Файл превышает лимит: {relative}");}
        total=total.saturating_add(entry.size());
        if total>2*1024*1024*1024 {bail!("Общий объём распаковки превышает 2 ГБ");}
        let target=output.join(&enclosed);
        fs::create_dir_all(target.parent().context("Wrong ZIP path")?)?;
        let mut out=File::create(&target)?;
        let mut hasher=Sha256::new();
        let mut copied=0u64;
        let mut buf=[0u8;64*1024];
        loop{
            let count=entry.read(&mut buf)?;
            if count==0{break;}
            copied=copied.saturating_add(count as u64);
            if copied>per_file_limit || copied>entry.size() {bail!("ZIP превышает заявленный размер");}
            out.write_all(&buf[..count])?;
            hasher.update(&buf[..count]);
            extracted=extracted.saturating_add(count as u64);
            on_progress(extracted,expected);
        }
        out.sync_all()?;
        files.push(SafeFile{relative,local:target,sha256:hex::encode(hasher.finalize()),size:copied});
    }
    if files.is_empty(){bail!("ZIP не содержит подходящих документов");}
    Ok(files)
}

/// The HTTP request only receives and durably registers the ZIP locally.
/// Extraction and NAS uploads happen in a detached Tokio task.
pub async fn public_upload(State(st):State<Shared>,Path(token):Path<String>,mut multipart:Multipart)->Response {
    match accept_upload(&st,&token,&mut multipart).await {
        Ok(receipt)=> {
            let mut response=(StatusCode::ACCEPTED,Json(json!({
                "accepted":true,"receipt":receipt,"status_url":format!("/contractor/{token}/status")
            }))).into_response();
            response.headers_mut().insert(header::CACHE_CONTROL,header::HeaderValue::from_static("no-store"));
            response
        },
        Err(err)=>{
            tracing::warn!("Contractor ZIP rejected: {err:#}");
            let message=err.to_string();
            let safe=if message.contains("Synology") || message.contains("DSM") || message.contains("database") || message.contains("pool") {
                "Не удалось принять архив. Обратитесь к ответственному сотруднику.".to_string()
            } else {message};
            (StatusCode::BAD_REQUEST,Json(json!({"error":safe}))).into_response()
        }
    }
}

async fn accept_upload(st:&Shared,token:&str,multipart:&mut Multipart)->Result<String>{
    let (package_id,project,work)=link_details(st,token).await
        .map_err(|_|anyhow::anyhow!("Не удалось проверить ссылку"))?
        .context("Ссылка недействительна или уже использована")?;
    // Keep multipart overhead below the existing 512 MiB reverse-proxy limit.
    // This is independent of MAX_FILE_MB, which limits each uncompressed file.
    let zip_max_mb=std::env::var("CONTRACTOR_ZIP_MAX_MB")
        .ok().and_then(|v|v.parse::<u64>().ok()).unwrap_or(480).clamp(1,500);
    let limit=zip_max_mb*1024*1024;
    // TempDir ownership moves into the worker. It must not be dropped when
    // the HTTP response completes, otherwise the incoming ZIP disappears.
    let staging=tempfile::tempdir_in(&st.cfg.temp_root)?;
    let mut archive=None;
    while let Some(mut field)=multipart.next_field().await? {
        if field.name()!=Some("archive") || archive.is_some(){bail!("Передайте ровно один ZIP-архив");}
        let name=field.file_name().unwrap_or("archive.zip").to_string();
        if !name.to_lowercase().ends_with(".zip") {bail!("Требуется ZIP-архив");}
        let path=staging.path().join("incoming.zip");
        let mut file=tokio::fs::File::create(&path).await?;
        let mut size=0u64;
        let mut digestor=Sha256::new();
        while let Some(chunk)=field.chunk().await? {
            size=size.saturating_add(chunk.len() as u64);
            if size>limit {bail!("ZIP превышает допустимый размер");}
            file.write_all(&chunk).await?;
            digestor.update(&chunk);
        }
        file.sync_all().await?;
        archive=Some((path,name,hex::encode(digestor.finalize())));
    }
    let (archive_path,archive_name,archive_hash)=archive.context("Не выбран архив")?;
    // Claim/allocate revision atomically BEFORE starting background work.
    let mut tx=st.db.begin().await?;
    let consumed=sqlx::query("UPDATE contractor_upload_links SET claimed_at=now() WHERE token_hash=$1 AND package_id=$2 AND claimed_at IS NULL AND revoked_at IS NULL AND expires_at>now() RETURNING id,nas_root")
        .bind(digest(token.as_bytes())).bind(package_id).fetch_optional(&mut *tx).await?
        .context("Ссылка уже использована другим запросом")?;
    let link_id:i64=consumed.try_get("id")?;
    let root:String=consumed.try_get("nas_root")?;
    validate_resdata_path(&root)?;
    sqlx::query("SELECT id FROM packages WHERE id=$1 FOR UPDATE")
        .bind(package_id).fetch_one(&mut *tx).await?;
    let revision:i32=sqlx::query_scalar("SELECT COALESCE(MAX(revision),0)+1 FROM contractor_submissions WHERE package_id=$1")
        .bind(package_id).fetch_one(&mut *tx).await?;
    let submission:i64=sqlx::query_scalar("INSERT INTO contractor_submissions(link_id,package_id,revision,archive_sha256,archive_filename,status,manifest_json,processing_stage) VALUES($1,$2,$3,$4,$5,'uploading','[]'::jsonb,'queued') RETURNING id")
        .bind(link_id).bind(package_id).bind(revision).bind(&archive_hash).bind(&archive_name)
        .fetch_one(&mut *tx).await?;
    tx.commit().await?;
    let receipt=format!("ИД-{package_id}-{revision:05}");
    let st=Arc::clone(st);
    let receipt_for_worker=receipt.clone();
    // Supervisor catches panics so a bad ZIP cannot leave a job permanently running.
    tokio::spawn(async move {
        let db=st.db.clone();
        let worker=tokio::spawn(async move {
            process_delivery(st,staging,archive_path,archive_hash,package_id,project,work,root,revision,submission).await
        });
        let result=match worker.await {
            Ok(result)=>result,
            Err(join)=>Err(anyhow::anyhow!("Worker terminated unexpectedly: {join}")),
        };
        match result {
            Ok(nas_path)=>{
                if let Err(err)=sqlx::query("UPDATE contractor_submissions SET status='pending_review',processing_stage='completed',processing_percent=100,processed_bytes=processing_total_bytes,nas_path=$2,progress_updated_at=now() WHERE id=$1 AND status='uploading'")
                    .bind(submission).bind(nas_path).execute(&db).await {
                    tracing::error!("Delivery {submission} finalization failed: {err:#}");
                } else {
                    let _=repo::audit(&db,"contractor_link","contractor_zip_uploaded",&format!("submission={submission} receipt={receipt_for_worker}")).await;
                }
            },
            Err(err)=>{
                tracing::error!("Delivery {submission} failed: {err:#}");
                let detail=err.to_string().chars().take(500).collect::<String>();
                let _=sqlx::query("UPDATE contractor_submissions SET status='failed',processing_stage='failed',error_message=$2,progress_updated_at=now() WHERE id=$1 AND status='uploading'")
                    .bind(submission).bind(detail).execute(&db).await;
            }
        }
    });
    Ok(receipt)
}

/// Abort the periodic PostgreSQL updater even if a processing future panics.
struct ReporterGuard(tokio::task::JoinHandle<()>);
impl Drop for ReporterGuard {fn drop(&mut self){self.0.abort();}}

struct ProgressCounters {
    stage:AtomicU8,done:AtomicU64,total:AtomicU64,
}
impl ProgressCounters {
    fn new()->Self {Self{stage:AtomicU8::new(0),done:AtomicU64::new(0),total:AtomicU64::new(1)}}
    fn phase(&self,phase:u8,total:u64) {
        self.done.store(0,Ordering::Relaxed);
        self.total.store(total.max(1),Ordering::Relaxed);
        self.stage.store(phase,Ordering::Release);
    }
}

async fn progress_reporter(db:sqlx::PgPool,submission:i64,p:Arc<ProgressCounters>){
    let mut interval=tokio::time::interval(std::time::Duration::from_millis(700));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let phase=p.stage.load(Ordering::Acquire);
        let bytes=p.done.load(Ordering::Relaxed);
        let total=p.total.load(Ordering::Relaxed).max(1);
        let fraction=bytes.min(total) as f64 / total as f64;
        let (label,percent)=match phase {
            0=>("queued",0),
            1=>("extracting",(fraction*25.0).floor() as i16),
            2=>("synology",25+(fraction*70.0).floor() as i16),
            3=>("publishing",96),
            _=>("publishing",96),
        };
        let _=sqlx::query("UPDATE contractor_submissions SET processing_stage=$2,processing_percent=$3,processed_bytes=$4,processing_total_bytes=$5,progress_updated_at=now() WHERE id=$1 AND status='uploading'")
            .bind(submission).bind(label).bind(percent).bind(bytes.min(i64::MAX as u64) as i64)
            .bind(total.min(i64::MAX as u64) as i64).execute(&db).await;
    }
}

async fn process_delivery(
    st:Shared,staging:tempfile::TempDir,archive_path:PathBuf,archive_hash:String,
    package_id:i64,project:String,work:String,root:String,revision:i32,submission:i64
)->Result<String>{
    let p=Arc::new(ProgressCounters::new());
    let reporter=ReporterGuard(tokio::spawn(progress_reporter(st.db.clone(),submission,p.clone())));
    let operation=async {
        p.phase(1,1);
        let target_dir=staging.path().join("extracted");
        fs::create_dir_all(&target_dir)?;
        let per_file_limit=st.cfg.max_file_mb*1024*1024;
        let unpack_path=archive_path.clone();
        let unpack_progress=p.clone();
        let files=tokio::task::spawn_blocking(move||{
            unpack(&unpack_path,&target_dir,per_file_limit,&|done,total| {
                unpack_progress.total.store(total.max(1),Ordering::Relaxed);
                unpack_progress.done.store(done,Ordering::Relaxed);
            })
        }).await??;
        let archive_len=fs::metadata(&archive_path)?.len();
        let total_sync=files.iter().map(|f|f.size).sum::<u64>().saturating_add(archive_len);
        p.phase(2,total_sync);
        let mut cfg=st.cfg.clone(); cfg.synology_root=root.clone();
        let nas=SynologyClient::new(&cfg)?;
        let session=nas.connect().await?;
        let receipt=uuid::Uuid::new_v4().simple().to_string();
        let published_name=format!("Поставка-{revision:05}-{}",&receipt[..12]);
        let base=format!("{root}/{project}/{}/ИД",nas_work_folder(&work));
        let stage_path=format!("{base}/.incoming-{receipt}");
        let final_path=format!("{base}/{published_name}");
        let uploaded=p.clone();
        let on_bytes:Arc<dyn Fn(u64)+Send+Sync>=Arc::new(move |count| {
            uploaded.done.fetch_add(count,Ordering::Relaxed);
        });
        // Atomic rename publishes a complete delivery to the scanner.
        let operation=async {
            for item in &files {
                let (parent,name)=match item.relative.rsplit_once('/') {
                    Some((folder,name))=>(format!("{stage_path}/{folder}"),name),
                    None=>(stage_path.clone(),item.relative.as_str()),
                };
                session.upload_file_with_progress(&parent,name,&item.local,Some(on_bytes.clone())).await?;
            }
            session.upload_file_with_progress(&stage_path,"original-package.zip",&archive_path,Some(on_bytes.clone())).await?;
            p.stage.store(3,Ordering::Release);
            session.publish_directory(&stage_path,&published_name).await?;
            Ok::<_,anyhow::Error>(())
        }.await;
        session.logout().await;
        operation?;
        let manifest:Vec<Value>=files.iter().map(|f|json!({"path":f.relative,"sha256":f.sha256,"size":f.size})).collect();
        sqlx::query("UPDATE contractor_submissions SET manifest_json=$2 WHERE id=$1")
            .bind(submission).bind(json!(manifest)).execute(&st.db).await?;
        tracing::info!("Contractor delivery {submission} published: sha256={archive_hash} package={package_id}");
        Ok::<_,anyhow::Error>(final_path)
    }.await;
    drop(reporter);
    operation
}

#[derive(Deserialize)]pub struct RevokeForm{csrf:String}
pub async fn revoke_link(State(st):State<Shared>,Extension(actor):Extension<Principal>,Path((package_id,link_id)):Path<(i64,i64)>,Form(form):Form<RevokeForm>)->std::result::Result<axum::response::Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    let role=access::package_role(&st,&actor,package_id).await.ok_or_else(WebError::forbidden)?;
    if role!="manager" && role!="engineer" {return Err(WebError::forbidden());}
    sqlx::query("UPDATE contractor_upload_links SET revoked_at=now() WHERE id=$1 AND package_id=$2 AND claimed_at IS NULL AND revoked_at IS NULL")
        .bind(link_id).bind(package_id).execute(&st.db).await?;
    repo::audit(&st.db,&actor_name(&actor),"contractor_link_revoked",&format!("package={package_id} link={link_id}")).await?;
    Ok(axum::response::Redirect::to(&format!("/packages/{package_id}#contractor")))
}

#[derive(Deserialize)]pub struct ReviewForm{csrf:String,action:String}
pub async fn review_submission(State(st):State<Shared>,Extension(actor):Extension<Principal>,Path((package_id,submission_id)):Path<(i64,i64)>,Form(form):Form<ReviewForm>)->std::result::Result<axum::response::Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    let role=access::package_role(&st,&actor,package_id).await.ok_or_else(WebError::forbidden)?;
    if role!="manager" && role!="engineer" {return Err(WebError::forbidden());}
    let info=sqlx::query("SELECT nas_path,status FROM contractor_submissions WHERE id=$1 AND package_id=$2")
        .bind(submission_id).bind(package_id).fetch_optional(&st.db).await?.ok_or_else(WebError::not_found)?;
    let status:String=info.try_get("status")?;
    if status!="pending_review" {return Err(WebError::bad("Этот комплект уже обработан"));}
    if form.action=="rework"{
        sqlx::query("UPDATE contractor_submissions SET status='rework',checked_by=$2,reviewed_at=now() WHERE id=$1 AND status='pending_review'")
            .bind(submission_id).bind(actor_name(&actor)).execute(&st.db).await?;
    }else if form.action=="check"{
        // Prevent a false 'checked' result before the scanner imports new files.
        let path:String=info.try_get::<Option<String>,_>("nas_path")?.context("NAS destination is missing")?;
        let prefix=path.rsplit('/').next().unwrap_or("");
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM documents WHERE package_id=$1 AND is_present=true AND relative_path LIKE $2 ESCAPE '\\'")
            .bind(package_id).bind(format!("ИД/{}/%",prefix.replace('\\',"\\\\").replace('%',"\\%").replace('_',"\\_"))).fetch_one(&st.db).await?;
        if count==0 {return Err(WebError::bad("Документы ещё не импортированы сканером Synology. Повторите проверку после сканирования"));}
        let updated=sqlx::query("UPDATE contractor_submissions SET status='checking' WHERE id=$1 AND status='pending_review'")
            .bind(submission_id).execute(&st.db).await?;
        if updated.rows_affected()!=1{return Err(WebError::bad("Поставка уже обрабатывается"));}
        let delivery_prefix=format!("ИД/{prefix}/");
        match engine::analyze_delivery(&st.db,&st.catalog,package_id,"contractor_review",&delivery_prefix).await{
            Ok(run)=>{
                sqlx::query("UPDATE contractor_submissions SET status='checked',check_run_id=$2,checked_by=$3,reviewed_at=now() WHERE id=$1")
                    .bind(submission_id).bind(run).bind(actor_name(&actor)).execute(&st.db).await?;
            },
            Err(err)=>{
                sqlx::query("UPDATE contractor_submissions SET status='pending_review',error_message=$2 WHERE id=$1")
                    .bind(submission_id).bind(err.to_string().chars().take(500).collect::<String>()).execute(&st.db).await?;
                return Err(WebError::from(err));
            }
        }
    }else{return Err(WebError::bad("Неизвестное решение"));}
    repo::audit(&st.db,&actor_name(&actor),"contractor_review",&format!("submission={submission_id} action={}",form.action)).await?;
    Ok(axum::response::Redirect::to(&format!("/packages/{package_id}#contractor")))
}

pub async fn package_controls(st:&Shared,actor:&Principal,package_id:i64)->Result<String>{
    let role=access::package_role(st,actor,package_id).await;
    if !matches!(role.as_deref(),Some("manager"|"engineer")){return Ok(String::new());}
    let rows=sqlx::query("SELECT revision,status,archive_sha256,nas_path,check_run_id,uploaded_at::text,id,error_message,processing_stage,processing_percent FROM contractor_submissions WHERE package_id=$1 ORDER BY revision DESC LIMIT 30")
        .bind(package_id).fetch_all(&st.db).await?;
    let mut html=format!("<section class='white-panel' id='contractor' style='padding:22px;margin:20px 0'><h2>Передача документов подрядчиком</h2><p>Сформируйте одноразовую ссылку для загрузки ZIP без Bitrix24. После загрузки комплект поступит на Synology и будет ждать проверки ответственного сотрудника.</p><form method='post' action='/packages/{package_id}/contractor-links'>{}<button class='primary'>Создать одноразовую ссылку (72 часа)</button></form>",web::hidden(&st.csrf));
    let open_links:Vec<(i64,String,String)>=sqlx::query_as("SELECT id,created_at::text,expires_at::text FROM contractor_upload_links WHERE package_id=$1 AND claimed_at IS NULL AND revoked_at IS NULL AND expires_at>now() ORDER BY id DESC LIMIT 20")
        .bind(package_id).fetch_all(&st.db).await?;
    if !open_links.is_empty(){
        html.push_str("<h3>Выданные ссылки</h3><table><tr><th>Дата создания</th><th>Действует до</th><th>Действие</th></tr>");
        for (link_id,created,expires) in open_links{
            html.push_str(&format!("<tr><td>{}</td><td>{}</td><td><form method='post' action='/packages/{package_id}/contractor-links/{link_id}/revoke'>{}<button class='secondary'>Отозвать ссылку</button></form></td></tr>",h(&created),h(&expires),web::hidden(&st.csrf)));
        }
        html.push_str("</table>");
    }
    html.push_str("<h3>Полученные комплекты и версии</h3><table><thead><tr><th>Поставка</th><th>Дата</th><th>Состояние</th><th>Контроль / действия</th></tr></thead><tbody>");
    for row in rows {
        let revision:i32=row.try_get("revision")?;
        let status:String=row.try_get("status")?;
        let receipt_id:i64=row.try_get("id")?;
        let uploaded:String=row.try_get("uploaded_at")?;
        let sha:String=row.try_get("archive_sha256")?;
        let run:Option<i64>=row.try_get("check_run_id")?;
        let (label,actions)=match status.as_str() {
            "pending_review"=>("Ожидает проверки",format!("<form method='post' action='/packages/{package_id}/submissions/{receipt_id}/review' class='inline'>{}<input type='hidden' name='action' value='check'><button>Запустить проверку</button></form> <form method='post' action='/packages/{package_id}/submissions/{receipt_id}/review' class='inline'>{}<input type='hidden' name='action' value='rework'><button class='secondary'>На доработку</button></form>",web::hidden(&st.csrf),web::hidden(&st.csrf))),
            "checked"=>("Проверено",run.map(|r|format!("<a href='/packages/{package_id}?run_id={r}#remarks'>Результаты проверки #{r}</a>")).unwrap_or_default()),
            "rework"=>("Возвращено на доработку",String::new()),
            "failed"=>("Ошибка доставки",String::new()),
            "uploading"=>("Приём / запись на Synology",String::new()),
            _=>("Проверяется",String::new()),
        };
        let stage:String=row.try_get("processing_stage")?;
        let percent:i16=row.try_get("processing_percent")?;
        let progress=if status=="uploading" {format!("<small style='display:block'>{} — {} %</small><progress max='100' value='{}' style='width:160px'></progress>",h(&stage),percent,percent)} else {String::new()};
        html.push_str(&format!("<tr><td><b>ИД-{package_id}-{revision:05}</b><small style='display:block'>SHA-256: {}…</small></td><td>{}</td><td>{}{}</td><td>{}</td></tr>",h(&sha[..12]),h(&uploaded),label,progress,actions));
    }
    html.push_str("</tbody></table></section>");
    Ok(html)
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn token_validation(){assert!(valid_token(&"a".repeat(64)));assert!(!valid_token("../x"));}
    #[test]fn safe_work_folder(){
        assert_eq!(nas_work_folder("Внутренние сети электроснабжения/электроосвещения"),"Внутренние сети электроснабжения - электроосвещения");
    }
    #[test]fn zip_rejects_escape_and_accepts_documents(){
        let temp=tempfile::tempdir().unwrap();
        let file=temp.path().join("demo.zip");
        let mut zip=zip::ZipWriter::new(File::create(&file).unwrap());
        zip.start_file("АОСР/акт.pdf",zip::write::FileOptions::default()).unwrap();
        zip.write_all(b"pdf").unwrap();zip.finish().unwrap();
        let dest=temp.path().join("out");
        let result=unpack(&file,&dest,1024,&|_,_|{}).unwrap();
        assert_eq!(result.len(),1);
    }
}
