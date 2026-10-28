//! Stage 2.3. Independent, one-use contractor links and safe DSM uploads.
//! No Bitrix cookie is required on public /contractor/:token routes.
use crate::{access::{self,Principal},engine,portal,repo,scanner,synology::{SynologyClient,validate_resdata_path},web::{self,Shared,WebError}};
use anyhow::{bail,Context,Result};
use encoding_rs::{IBM866, WINDOWS_1251};
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
fn html_attribute(s:&str)->String{html_escape::encode_double_quoted_attribute(s).to_string()}
/// The catalogue includes work names containing '/': never use these verbatim
/// as NAS path components. A stable human-readable folder name is sufficient.
pub fn nas_work_folder(name:&str)->String{name.replace('/'," - ")}

/// Human-readable DSM delivery folder. `submission_id` comes from the global
/// BIGSERIAL sequence in PostgreSQL; it is not a per-package revision and is
/// never derived from a random hash. The date is the submission date in
/// Astrakhan local time (UTC+04:00), not the date the background worker ends.
fn delivery_folder_name(work:&str, submission_id:i64, uploaded_at:chrono::DateTime<chrono::Utc>)->Result<String>{
    if submission_id<=0 {bail!("Invalid global delivery number");}
    // Names in the work catalogue may contain slashes or other characters
    // forbidden by DSM/Windows. Limit the label so the full segment fits NAS.
    let mut label=String::new();
    for ch in work.chars().take(60) {
        let safe=if ch.is_alphanumeric() || matches!(ch,' ' | '-' | '_' | '(' | ')' | '.') {ch} else {'-'};
        label.push(safe);
    }
    let label=label.trim_matches(|c:char|c.is_whitespace() || c=='-' || c=='.');
    if label.is_empty(){bail!("Work title cannot be used as a delivery folder");}
    let tz=chrono::FixedOffset::east_opt(4*3600).context("Invalid delivery timezone")?;
    let date=uploaded_at.with_timezone(&tz).format("%d.%m.%y");
    let folder=format!("{label}-версия-{submission_id}-{date}");
    repo::validate_segment(&folder)?;
    Ok(folder)
}

/// Legacy names remain valid for old submissions. New names are validated
/// separately so existing imports/review links keep working after upgrade.
pub fn is_published_delivery_folder(name:&str)->bool {
    if repo::validate_segment(name).is_err(){return false;}
    if name.starts_with("Поставка-"){return true;}
    let Some((prefix,date))=name.rsplit_once('-') else {return false;};
    let Some((title,number))=prefix.rsplit_once("-версия-") else {return false;};
    if title.is_empty() || number.is_empty() || !number.as_bytes().iter().all(u8::is_ascii_digit) ||
       number.parse::<i64>().map(|n|n<=0).unwrap_or(true) {return false;}
    let Ok(parsed)=chrono::NaiveDate::parse_from_str(date,"%d.%m.%y") else {return false;};
    parsed.format("%d.%m.%y").to_string()==date
}

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

/// A directory can be opened only if it is an immediate child of the directory
/// currently displayed. This protects the browser against unexpected DSM paths.
fn direct_subdirectory(parent:&str,child:&str)->bool {
    validate_resdata_path(parent).is_ok() && validate_resdata_path(child).is_ok() &&
    child.rsplit_once('/').map(|(base,name)|base==parent && !name.is_empty()).unwrap_or(false)
}

/// Build links for each ancestor of a validated path inside the ResData share.
fn root_breadcrumbs(folder:&str)->Vec<(String,String)> {
    let mut items=vec![("/ResData".to_string(),"ResData".to_string())];
    let mut path=String::from("/ResData");
    if let Some(remainder)=folder.strip_prefix("/ResData/") {
        for part in remainder.split('/') {
            path.push('/');path.push_str(part);
            items.push((path.clone(),part.to_string()));
        }
    }
    items
}

fn folder_href(path:&str)->String {
    format!("/administration/storage?folder={}",url::form_urlencoded::byte_serialize(path.as_bytes()).collect::<String>())
}

#[derive(Deserialize)]pub struct Browse {folder:Option<String>,saved:Option<String>}
pub async fn storage_view(State(st):State<Shared>,Extension(actor):Extension<Principal>,Query(params):Query<Browse>)->std::result::Result<Html<String>,WebError>{
    if !actor.superuser {return Err(WebError::forbidden());}
    let current=selected_root(&st).await?;
    // Starting at /ResData makes the whole directory tree discoverable.
    let folder=params.folder.unwrap_or_else(||"/ResData".to_string());
    validate_resdata_path(&folder).map_err(|_|WebError::bad("Разрешены только папки внутри /ResData"))?;
    let session=st.nas.connect().await.map_err(|err|WebError::bad(format!("Нет соединения с Synology: {err:#}")))?;
    let dirs=session.list_directories(&folder).await;
    session.logout().await;
    let dirs=dirs.map_err(|err|WebError::bad(format!("Не удалось открыть папку Synology: {err:#}")))?;
    let latest:Option<(i64,String)>=sqlx::query_as(
        "SELECT version,root_path FROM synology_root_versions ORDER BY version DESC LIMIT 1"
    ).fetch_optional(&st.db).await?;
    let history:Vec<(i64,String,chrono::DateTime<chrono::Utc>)>=sqlx::query_as(
        "SELECT version,root_path,changed_at FROM synology_root_versions ORDER BY version DESC LIMIT 10"
    ).fetch_all(&st.db).await?;
    let rev=latest.map(|(v,_)|format!("v{v}")).unwrap_or_else(||"Из .env".to_string());
    let mut body=format!(r#"<section class="storage-explorer">
      <div class="page-title"><h1>Папка проектов на Synology</h1><p>Открывайте папки внутри /ResData одним нажатием. Когда найдёте нужный каталог, нажмите «Выбрать как корень проектов».</p></div>
      <div class="storage-root-summary"><div><small>Корень для новых поставок · {}</small><strong><code>{}</code></strong><span>Проекты и виды работ создаются внутри этого пути автоматически. Предыдущие поставки и версии документов сохраняются.</span></div><a class="buttonlink" href="{}">Открыть текущий корень</a></div>"#,
      h(&rev),h(&current),folder_href(if validate_resdata_path(&current).is_ok(){&current}else{"/ResData"}));
    if params.saved.as_deref()==Some("1") {
        body.push_str("<p class=\"storage-success\" role=\"status\">Корень сохранён. Следующие ссылки подрядчикам будут использовать новую папку; существующие ссылки и версии не меняются.</p>");
    } else if params.saved.as_deref()==Some("unchanged") {
        body.push_str("<p class=\"storage-success\" role=\"status\">Эта папка уже выбрана корнем. Настройки не изменились.</p>");
    }
    body.push_str("<section class=\"white-panel storage-picker\"><div class=\"storage-picker-heading\"><div><h2>Проводник ResData</h2><p>Только папки, без файлов. Выбор доступен суперадминистратору.</p></div><a class=\"storage-home\" href=\"/administration/storage?folder=%2FResData\">К началу ResData</a></div>");
    body.push_str("<nav class=\"storage-breadcrumbs\" aria-label=\"Путь к папке\">");
    let breadcrumbs=root_breadcrumbs(&folder);
    for (idx,(path,title)) in breadcrumbs.iter().enumerate() {
        if idx>0 {body.push_str("<span aria-hidden=\"true\">›</span>");}
        if idx==breadcrumbs.len()-1 {
            body.push_str(&format!("<strong aria-current=\"location\">{}</strong>",h(title)));
        } else {
            body.push_str(&format!("<a href=\"{}\">{}</a>",folder_href(path),h(title)));
        }
    }
    body.push_str("</nav>");
    body.push_str(&format!(r#"<div class="storage-location"><div><small>Открытая папка</small><code>{}</code></div><form method="post" action="/administration/storage">{}<input type="hidden" name="root" value="{}"><button class="primary" type="submit">{} Выбрать как корень проектов</button></form></div>"#,
        h(&folder),web::hidden(&st.csrf),html_attribute(&folder),if folder==current{"✓"}else{"＋"}));
    body.push_str("<div class=\"storage-directory-list\">");
    if folder!="/ResData" {
        let parent=folder.rsplit_once('/').map(|(p,_)|p).unwrap_or("/ResData");
        body.push_str(&format!("<a class=\"storage-directory storage-up\" href=\"{}\"><span aria-hidden=\"true\">↰</span><span>На уровень выше<small>{}</small></span></a>",folder_href(parent),h(parent)));
    }
    let mut n=0usize;
    for child in dirs {
        if !direct_subdirectory(&folder,&child) {continue;}
        n+=1;
        let name=child.rsplit('/').next().unwrap_or(&child);
        body.push_str(&format!("<a class=\"storage-directory\" href=\"{}\"><span aria-hidden=\"true\">📁</span><span>{}<small>Открыть папку</small></span><span aria-hidden=\"true\">›</span></a>",folder_href(&child),h(name)));
    }
    if n==0 {body.push_str("<p class=\"storage-empty\">Вложенных папок нет. Можно выбрать текущую папку для проектов.</p>");}
    body.push_str("</div></section>");
    body.push_str("<section class=\"white-panel storage-history\"><h2>История выбора корневой папки</h2><p>Версия настройки сохраняется при смене каталога. История поставок по каждому виду работ остаётся независимой от выбранного пути.</p><div class=\"storage-revisions\">");
    if history.is_empty() {body.push_str("<p>Корневая папка ещё не выбиралась через приложение.</p>");}
    for (version,path,date) in history {
        let note=if path==current {" · текущая"}else{""};
        body.push_str(&format!("<div class=\"storage-revision\"><strong>v{}{}</strong><code>{}</code><small>{}</small><a href=\"{}\">Посмотреть папку</a></div>",
            version,note,h(&path),date.format("%d.%m.%Y %H:%M UTC"),folder_href(&path)));
    }
    body.push_str("</div><p class=\"storage-version-help\">При загрузке нового ZIP ссылка подрядчика запоминает корень на момент создания. Для каждой следующей поставки увеличивается номер ревизии; предыдущие документы и исходные ZIP сохраняются.</p></section><p><a href=\"/administration\">← К администрированию</a></p></section>");
    Ok(web::layout("Папки Synology",body))
}
#[derive(Deserialize)]pub struct RootForm{csrf:String,root:String}
pub async fn storage_save(State(st):State<Shared>,Extension(actor):Extension<Principal>,Form(form):Form<RootForm>)->std::result::Result<axum::response::Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    if !actor.superuser{return Err(WebError::forbidden());}
    validate_resdata_path(&form.root).map_err(|_|WebError::bad("Папка должна быть внутри /ResData"))?;
    // Verify that DSM can open the requested folder; never trust the posted path.
    let session=st.nas.connect().await.map_err(|e|WebError::bad(format!("DSM: {e:#}")))?;
    let checked=session.list_directories(&form.root).await;
    session.logout().await;
    checked.map_err(|e|WebError::bad(format!("DSM не подтверждает доступ к папке: {e:#}")))?;

    // One transaction serializes concurrent superadmin updates so root versions
    // are never duplicated. Old contractor links keep their nas_root snapshots.
    let mut tx=st.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(48213422)").execute(&mut *tx).await?;
    let previous=sqlx::query_scalar::<_,String>("SELECT setting_value FROM app_settings WHERE setting_key='synology_root'")
        .fetch_optional(&mut *tx).await?;
    let changed=previous.as_deref()!=Some(form.root.as_str());
    if changed {
        sqlx::query("INSERT INTO app_settings(setting_key,setting_value) VALUES('synology_root',$1) ON CONFLICT(setting_key) DO UPDATE SET setting_value=EXCLUDED.setting_value,updated_at=now()")
            .bind(&form.root).execute(&mut *tx).await?;
        let version:i64=sqlx::query_scalar("SELECT COALESCE(MAX(version),0)+1 FROM synology_root_versions")
            .fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO synology_root_versions(version,root_path,changed_by) VALUES($1,$2,$3)")
            .bind(version).bind(&form.root).bind(actor_name(&actor)).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    if changed {repo::audit(&st.db,&actor_name(&actor),"synology_root_changed",&form.root).await?;}
    let query=url::form_urlencoded::byte_serialize(form.root.as_bytes()).collect::<String>();
    Ok(axum::response::Redirect::to(&format!("/administration/storage?folder={query}&saved={}",if changed{"1"}else{"unchanged"})))
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

/// Decode names from ZIP archives created by Windows tools.
/// The zip crate correctly handles UTF-8 but otherwise assumes CP437; old Russian
/// ZIPs commonly store filenames in CP866 without the UTF-8 flag.
/// A different legacy encoding can be selected with CONTRACTOR_ZIP_LEGACY_ENCODING=cp1251.
fn decode_zip_path(raw:&[u8])->Result<(String,PathBuf)> {
    if raw.is_empty() || raw.len()>4096 { bail!("Некорректная длина имени в ZIP"); }
    let decoded=match std::str::from_utf8(raw) {
        Ok(value)=>value.to_owned(),
        Err(_)=>{
            let charset=std::env::var("CONTRACTOR_ZIP_LEGACY_ENCODING")
                .unwrap_or_else(|_|"cp866".to_owned()).to_ascii_lowercase();
            let encoding=match charset.as_str(){
                "cp866"|"ibm866"|"866"=>IBM866,
                "cp1251"|"windows-1251"|"1251"=>WINDOWS_1251,
                _=>bail!("Недопустимая кодировка CONTRACTOR_ZIP_LEGACY_ENCODING"),
            };
            let (text,had_errors)=encoding.decode_without_bom_handling(raw);
            if had_errors {bail!("Не удалось расшифровать имя в ZIP");}
            text.into_owned()
        }
    };
    // Validate all path segments *after decoding*. Reject traversal, absolute
    // paths, drive letters, hidden components and control characters.
    let normal=decoded.replace('\\',"/");
    let segments=normal.split('/').collect::<Vec<_>>();
    if segments.is_empty() || segments.len()>12 ||
        segments.iter().any(|s|repo::validate_segment(s).is_err() || s.contains(':')) {
        bail!("Некорректное имя или путь файла в ZIP");
    }
    let relative=segments.join("/");
    Ok((relative.clone(),PathBuf::from(relative)))
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
        let (relative,enclosed)=decode_zip_path(entry.name_raw())?;
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
            4=>("importing",96+(fraction*3.0).floor() as i16),
            _=>("publishing",96),
        };
        let _=sqlx::query("UPDATE contractor_submissions SET processing_stage=$2,processing_percent=$3,processed_bytes=$4,processing_total_bytes=$5,progress_updated_at=now() WHERE id=$1 AND status='uploading'")
            .bind(submission).bind(label).bind(percent).bind(bytes.min(i64::MAX as u64) as i64)
            .bind(total.min(i64::MAX as u64) as i64).execute(&db).await;
    }
}

// Register a published contractor revision immediately. We already have the
// unpacked originals locally, so waiting for the 45-second NAS scanner would
// leave an apparently ready submission with zero documents in PostgreSQL.
// The same routine is used to recover older, already-published revisions.
async fn register_delivery_files(
    st:&Shared,package_id:i64,delivery_folder:&str,
    files:&[SafeFile],progress:Option<&ProgressCounters>
)->Result<()> {
    // The one-use link is the authoritative binding to this exact package.
    repo::validate_segment(delivery_folder)?;
    // Serialise delivery indexing with the periodic scanner to avoid version
    // races on the same (package_id, relative_path).
    let _guard=st.scan_lock.lock().await;
    for (idx,item) in files.iter().enumerate() {
        let (relative,_)=decode_zip_path(item.relative.as_bytes())?;
        let expected=format!("ИД/{delivery_folder}/{relative}");
        scanner::ingest_published(
            &st.db,&st.cfg,&st.catalog,package_id,&item.local,&expected
        ).await?;
        // ingest_published() returns None for an already-indexed document.
        // Ensure the active row exists; otherwise the check must not start.
        let exists:i64=sqlx::query_scalar(
            "SELECT count(*) FROM documents WHERE package_id=$1 AND relative_path=$2 AND is_present=true"
        ).bind(package_id).bind(&expected).fetch_one(&st.db).await?;
        if exists==0 {bail!("File was not registered in documents: {expected}");}
        if let Some(progress)=progress {
            progress.done.store((idx+1) as u64,Ordering::Relaxed);
        }
    }
    Ok(())
}

fn file_sha256(path:&FsPath)->Result<String> {
    let mut source=File::open(path)?;
    let mut hasher=Sha256::new();
    let mut buf=[0u8;64*1024];
    loop {
        let n=source.read(&mut buf)?;
        if n==0 {break;}
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

// For revisions published by previous releases, recover documents using the
// NAS path frozen on the one-use link; never use a newly selected /ResData root.
async fn import_existing_delivery(
    st:&Shared,package_id:i64,submission_id:i64,nas_path:&str
)->Result<()> {
    let row=sqlx::query(
        "SELECT l.nas_root,s.manifest_json,p.name AS project,k.name AS work \
         FROM contractor_submissions s \
         JOIN contractor_upload_links l ON l.id=s.link_id \
         JOIN packages k ON k.id=s.package_id \
         JOIN projects p ON p.id=k.project_id \
         WHERE s.id=$1 AND s.package_id=$2"
    ).bind(submission_id).bind(package_id).fetch_one(&st.db).await?;
    let root:String=row.try_get("nas_root")?;
    let project:String=row.try_get("project")?;
    let work:String=row.try_get("work")?;
    let manifest:Value=row.try_get("manifest_json")?;
    let entries=manifest.as_array().context("Missing submission manifest")?;
    if entries.is_empty(){bail!("Delivery manifest contains no files");}
    validate_resdata_path(&root)?;
    let folder=nas_path.rsplit('/').next().context("Missing published delivery folder")?;
    repo::validate_segment(folder)?;
    let expected=format!("{root}/{project}/{}/ИД/{folder}",nas_work_folder(&work));
    if nas_path!=expected {bail!("Published submission path does not match the original upload link");}
    let mut cfg=st.cfg.clone();
    cfg.synology_root=root;
    let nas=SynologyClient::new(&cfg)?;
    let session=nas.connect().await?;
    let temporary=tempfile::tempdir_in(&st.cfg.temp_root)?;
    let mut files=Vec::with_capacity(entries.len());
    let result=async {
        for entry in entries {
            let name=entry.get("path").and_then(Value::as_str).context("Manifest file path missing")?;
            let (_,relative)=decode_zip_path(name.as_bytes())?;
            let filename=relative.to_str().context("Invalid manifest path")?;
            let size=entry.get("size").and_then(Value::as_u64).context("Manifest file size missing")?;
            let checksum=entry.get("sha256").and_then(Value::as_str).context("Manifest checksum missing")?;
            if size>cfg.max_file_mb*1024*1024 {bail!("Manifest file exceeds MAX_FILE_MB");}
            if checksum.len()!=64 || !checksum.as_bytes().iter().all(|b|b.is_ascii_hexdigit()) {
                bail!("Invalid manifest checksum");
            }
            let local=temporary.path().join(&relative);
            fs::create_dir_all(local.parent().context("Bad manifest path")?)?;
            session.download_to(
                &format!("{nas_path}/{filename}"),&local,cfg.max_file_mb*1024*1024
            ).await?;
            if fs::metadata(&local)?.len()!=size || file_sha256(&local)?!=checksum {
                bail!("NAS file does not match original delivery manifest: {filename}");
            }
            files.push(SafeFile{
                relative:filename.to_owned(),local,
                sha256:checksum.to_owned(),size,
            });
        }
        Ok::<(),anyhow::Error>(())
    }.await;
    session.logout().await;
    result?;
    register_delivery_files(st,package_id,folder,&files,None).await?;
    tracing::info!(submission_id,package_id,files=files.len(),"Indexed an existing contractor delivery from Synology");
    Ok(())
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
        let submission_row=sqlx::query(
            "SELECT uploaded_at,archive_filename FROM contractor_submissions WHERE id=$1 AND package_id=$2"
        ).bind(submission).bind(package_id).fetch_one(&st.db).await?;
        let uploaded_at:chrono::DateTime<chrono::Utc>=submission_row.try_get("uploaded_at")?;
        let archive_filename:String=submission_row.try_get("archive_filename")?;
        // One global sequence across all projects and work types. Keep the
        // package-local `revision` for receipts and historical comparisons.
        let published_name=delivery_folder_name(&work,submission,uploaded_at)?;
        let manifest:Vec<Value>=files.iter().map(|f|json!({"path":f.relative,"sha256":f.sha256,"size":f.size})).collect();
        // The archive checksum no longer appears in the directory name.
        // Save it inside the published delivery with all per-file hashes.
        let metadata_path=staging.path().join("delivery-info.json");
        let metadata=serde_json::to_vec_pretty(&json!({
            "schema_version":1,
            "folder":published_name,
            "global_version":submission,
            "package_revision":revision,
            "package_id":package_id,
            "project":project,
            "work_type":work,
            "uploaded_at":uploaded_at.to_rfc3339(),
            "archive_filename":archive_filename,
            "archive_sha256":archive_hash,
            "files":manifest
        }))?;
        fs::write(&metadata_path,&metadata)?;
        let archive_len=fs::metadata(&archive_path)?.len();
        let total_sync=files.iter().map(|f|f.size).sum::<u64>()
            .saturating_add(archive_len).saturating_add(metadata.len() as u64);
        p.phase(2,total_sync);
        let mut cfg=st.cfg.clone(); cfg.synology_root=root.clone();
        let nas=SynologyClient::new(&cfg)?;
        let session=nas.connect().await?;
        let receipt=uuid::Uuid::new_v4().simple().to_string();
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
            session.upload_file_with_progress(&stage_path,"delivery-info.json",&metadata_path,Some(on_bytes.clone())).await?;
            p.stage.store(3,Ordering::Release);
            session.publish_directory(&stage_path,&published_name).await?;
            Ok::<_,anyhow::Error>(())
        }.await;
        session.logout().await;
        operation?;
        // Persist NAS location before OCR/indexing: a subsequent review or retry
        // can recover published files without asking the contractor to re-upload.
        sqlx::query("UPDATE contractor_submissions SET manifest_json=$2,nas_path=$3 WHERE id=$1")
            .bind(submission).bind(json!(manifest)).bind(&final_path).execute(&st.db).await?;
        p.phase(4,files.len() as u64);
        register_delivery_files(
            &st,package_id,&published_name,&files,Some(&p)
        ).await?;
        tracing::info!("Contractor delivery {submission} published and indexed: sha256={archive_hash} package={package_id}");
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
    if status!="pending_review" && !(status=="failed" && form.action=="check" && info.try_get::<Option<String>,_>("nas_path")?.is_some()) {
        return Err(WebError::bad("Этот комплект уже обработан"));
    }
    if form.action=="rework"{
        sqlx::query("UPDATE contractor_submissions SET status='rework',checked_by=$2,reviewed_at=now() WHERE id=$1 AND status='pending_review'")
            .bind(submission_id).bind(actor_name(&actor)).execute(&st.db).await?;
    }else if form.action=="check"{
        let path:String=info.try_get::<Option<String>,_>("nas_path")?
            .context("NAS destination is missing")?;
        let previous_status=status.clone();
        let updated=sqlx::query(
            "UPDATE contractor_submissions SET status='checking',error_message=NULL              WHERE id=$1 AND package_id=$2 AND status=$3"
        ).bind(submission_id).bind(package_id).bind(&previous_status)
         .execute(&st.db).await?;
        if updated.rows_affected()!=1 {return Err(WebError::bad("Поставка уже обрабатывается"));}
        let worker_st=Arc::clone(&st);
        let checked_by=actor_name(&actor);
        tokio::spawn(async move {
            let operation=async {
                let prefix=path.rsplit('/').next().context("Missing NAS delivery folder")?;
                repo::validate_segment(prefix)?;
                let relative_prefix=format!("ИД/{prefix}/");
                let row=sqlx::query(
                    "SELECT manifest_json FROM contractor_submissions WHERE id=$1 AND package_id=$2"
                ).bind(submission_id).bind(package_id).fetch_one(&worker_st.db).await?;
                let manifest:Value=row.try_get("manifest_json")?;
                let expected=manifest.as_array().context("Missing delivery manifest")?.len() as i64;
                if expected==0 {bail!("Delivery manifest contains no files");}
                let present:i64=sqlx::query_scalar(
                    "SELECT count(DISTINCT relative_path) FROM documents                      WHERE package_id=$1 AND source_kind='ID' AND is_present=true                      AND relative_path LIKE $2 ESCAPE '\\'"
                ).bind(package_id).bind(format!(
                    "{}%",relative_prefix.replace('\\',"\\\\").replace('%',"\\%").replace('_',"\\_")
                )).fetch_one(&worker_st.db).await?;
                if present<expected {
                    import_existing_delivery(&worker_st,package_id,submission_id,&path).await?;
                }
                let run=engine::analyze_delivery(
                    &worker_st.db,&worker_st.catalog,package_id,"contractor_review",&relative_prefix
                ).await?;
                sqlx::query(
                    "UPDATE contractor_submissions SET status='checked',check_run_id=$2,checked_by=$3,reviewed_at=now()                      WHERE id=$1 AND status='checking'"
                ).bind(submission_id).bind(run).bind(&checked_by)
                 .execute(&worker_st.db).await?;
                Ok::<(),anyhow::Error>(())
            }.await;
            if let Err(err)=operation {
                tracing::error!(submission_id,"Contractor review failed: {err:#}");
                let message=err.to_string().chars().take(500).collect::<String>();
                let _=sqlx::query(
                    "UPDATE contractor_submissions SET status='pending_review',error_message=$2                      WHERE id=$1 AND status='checking'"
                ).bind(submission_id).bind(message).execute(&worker_st.db).await;
            }
        });
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
        let delivery_nas_path:Option<String>=row.try_get("nas_path")?;
        let error_message:Option<String>=row.try_get("error_message")?;
        let run:Option<i64>=row.try_get("check_run_id")?;
        let (label,actions)=match status.as_str() {
            "pending_review"=>("Ожидает проверки",format!("<form method='post' action='/packages/{package_id}/submissions/{receipt_id}/review' class='inline'>{}<input type='hidden' name='action' value='check'><button>Запустить проверку</button></form> <form method='post' action='/packages/{package_id}/submissions/{receipt_id}/review' class='inline'>{}<input type='hidden' name='action' value='rework'><button class='secondary'>На доработку</button></form>",web::hidden(&st.csrf),web::hidden(&st.csrf))),
            "checked"=>("Проверено",run.map(|r|format!("<a href='/packages/{package_id}?run_id={r}#remarks'>Результаты проверки #{r}</a>")).unwrap_or_default()),
            "rework"=>("Возвращено на доработку",String::new()),
            "checking"=>("Импорт документов и проверка",format!("<a href='/packages/{package_id}#contractor'>Обновить состояние</a>")),
            "failed"=>{
                let nas_path:Option<String>=row.try_get("nas_path")?;
                let action=if nas_path.is_some() {
                    format!("<form method='post' action='/packages/{package_id}/submissions/{receipt_id}/review' class='inline'>{}<input type='hidden' name='action' value='check'><button>Повторить импорт и проверку</button></form>",web::hidden(&st.csrf))
                }else {String::new()};
                ("Ошибка доставки / обработки",action)
            },
            "uploading"=>("Приём / запись на Synology",String::new()),
            _=>("Проверяется",String::new()),
        };
        let stage:String=row.try_get("processing_stage")?;
        let percent:i16=row.try_get("processing_percent")?;
        let progress=if status=="uploading" {format!("<small style='display:block'>{} — {} %</small><progress max='100' value='{}' style='width:160px'></progress>",h(&stage),percent,percent)} else {String::new()};
        // Show the stored cause in the employee interface, escaped as HTML.
        let failure=if status=="failed" || status=="pending_review" {
            error_message.as_deref().filter(|msg|!msg.trim().is_empty())
                .map(|msg|format!("<small style='display:block;max-width:320px;overflow-wrap:anywhere'>Причина: {}</small>",h(msg)))
                .unwrap_or_default()
        } else {String::new()};
        let folder_label=delivery_nas_path.as_deref().and_then(|path|path.rsplit('/').next())
            .map(|name|format!("<small style='display:block'>Папка: {}</small>",h(name)))
            .unwrap_or_default();
        html.push_str(&format!("<tr><td><b>ИД-{package_id}-{revision:05}</b>{folder_label}<small style='display:block'>SHA-256: {}…</small></td><td>{}</td><td>{}{}{}</td><td>{}</td></tr>",h(&sha[..12]),h(&uploaded),label,progress,failure,actions));
    }
    html.push_str("</tbody></table></section>");
    Ok(html)
}

#[cfg(test)]mod tests{
    use super::*;
    #[test]fn token_validation(){assert!(valid_token(&"a".repeat(64)));assert!(!valid_token("../x"));}
    #[test]fn folder_navigation_is_restricted_to_immediate_children(){
        assert!(direct_subdirectory("/ResData","/ResData/ПТО"));
        assert!(direct_subdirectory("/ResData/ПТО","/ResData/ПТО/ИД"));
        assert!(!direct_subdirectory("/ResData","/ResData/ПТО/ИД"));
        assert!(!direct_subdirectory("/ResData/ПТО","/ResData/other"));
        assert!(!direct_subdirectory("/ResData/ПТО","/ResData/ПТО/../secret"));
    }
    #[test]fn breadcrumb_preserves_nested_paths(){
        let crumbs=root_breadcrumbs("/ResData/ПТО/ИД/ЖК");
        assert_eq!(crumbs.len(),4);
        assert_eq!(crumbs[2],("/ResData/ПТО/ИД".to_string(),"ИД".to_string()));
        assert_eq!(crumbs[3].0,"/ResData/ПТО/ИД/ЖК");
    }
    #[test]fn safe_work_folder(){
        assert_eq!(nas_work_folder("Внутренние сети электроснабжения/электроосвещения"),"Внутренние сети электроснабжения - электроосвещения");
    }
    #[test]fn new_delivery_folder_uses_global_number_and_upload_date(){
        use chrono::{TimeZone,Utc};
        let date=Utc.with_ymd_and_hms(2026,10,26,21,40,0).single().unwrap();
        let folder=delivery_folder_name("ЗУ",123,date).unwrap();
        assert_eq!(folder,"ЗУ-версия-123-27.10.26");
        assert!(is_published_delivery_folder(&folder));
        assert!(is_published_delivery_folder("Поставка-00001-6cd3ad7f7e60"));
        assert!(!is_published_delivery_folder("ЗУ-версия-x-26.10.26"));
        assert!(!is_published_delivery_folder("ЗУ-версия-1-31.02.26"));
    }
    #[test]fn folder_label_escapes_slashes_and_rejects_empty(){
        use chrono::{TimeZone,Utc};
        let date=Utc.with_ymd_and_hms(2026,10,26,10,0,0).single().unwrap();
        let folder=delivery_folder_name("Сети связи/сигнализация",4,date).unwrap();
        assert_eq!(folder,"Сети связи-сигнализация-версия-4-26.10.26");
        assert!(delivery_folder_name("/",4,date).is_err());
    }
    #[test]fn cp866_legacy_zip_name_decodes_to_cyrillic(){
        let raw=b"\x88\x84/1_\x91\xa2\xa0\xa8/1_\x92\xa8\xe2\xe3\xab.doc";
        let (name,path)=decode_zip_path(raw).unwrap();
        assert_eq!(name,"ИД/1_Сваи/1_Титул.doc");
        assert_eq!(path,PathBuf::from("ИД/1_Сваи/1_Титул.doc"));
        assert!(crate::docs::supported(&path));
    }
    #[test]fn decoded_zip_names_block_traversal_and_executables(){
        for bad in [b"../passwd".as_slice(),b"/etc/passwd",b"C:/windows/a.doc",b"dir/../../a.doc",b"abc\\..\\evil.doc"] {
            assert!(decode_zip_path(bad).is_err());
        }
        assert!(!crate::docs::supported(FsPath::new("photo.exe")));
        assert!(crate::docs::supported(FsPath::new("old.doc")));
        assert!(crate::docs::supported(FsPath::new("old.xls")));
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
