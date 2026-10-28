//! Shared project design library: upload once for an entire ЖК, automatically
//! expose each current revision as a REFERENCE to every work package.
//! Never classify these files as contractor ID or create a contractor delivery.
use crate::{access::{self,Principal},contractor,docs,repo,web::{self,Shared,WebError}};
use anyhow::{Context,Result};
use axum::{body::Body,extract::{Multipart,Path,State},Extension,response::{Redirect,Response},http::header,Form};
use serde::Deserialize;
use sha2::{Digest,Sha256};
use sqlx::{PgPool,Row};
use std::{collections::HashSet,io::{Read,Write},path::Path as FsPath};

pub const CATEGORIES:&[(&str,&str)]=&[
 ("KR","КР — конструктивные решения"),
 ("PZU","ПЗУ — планировка земельного участка"),
 ("POS","ПОС — организация строительства"),
 ("PPR","ППР — проект производства работ"),
 ("PD","Прочая проектная документация"),
 ("RD","Рабочая документация"),
 ("SURVEYS","Инженерные изыскания"),
 ("NTD","Нормативные документы (ГОСТ, СП)"),
 ("OTHER","Другие исходные документы"),
];
fn label(category:&str)->&'static str {CATEGORIES.iter().find(|(id,_)|*id==category).map(|x|x.1).unwrap_or("Исходный документ")}
fn h(s:&str)->String {html_escape::encode_safe(s).to_string()}
fn who(actor:&Principal)->String {format!("bitrix:{}:{}",actor.member,actor.user_id)}
fn doc_rel(category:&str,filename:&str)->String {
    let kind=if category=="RD"{"RD"}else if category=="NTD"{"NTD"}else{"PD"};
    format!("REFERENCE/{kind}/PROJECT/{category}/{filename}")
}
fn ref_category(category:&str)->&str {if category=="RD"{"RD"}else if category=="NTD"{"NTD"}else{"PD"}}

/// Project manager or explicitly assigned engineer may maintain project sources.
/// Regular users and contractors cannot modify these documents.
pub async fn can_upload(st:&Shared,a:&Principal,project_id:i64)->bool {
    if access::is_manager(st,a,project_id).await{return true;}
    sqlx::query_scalar::<_,bool>(
      "SELECT EXISTS(SELECT 1 FROM bitrix_package_acl a JOIN packages p ON p.id=a.package_id WHERE p.project_id=$1 AND a.member_id=$2 AND a.user_id=$3 AND a.role='engineer')")
      .bind(project_id).bind(&a.member).bind(a.user_id).fetch_one(&st.db).await.unwrap_or(false)
}

/// Renders a project-wide library on the project page. No extra service or iframe.
pub async fn panel(st:&Shared,a:&Principal,project_id:i64)->Result<String,WebError> {
    let allowed=can_upload(st,a,project_id).await;
    let sources=sqlx::query("SELECT id,category,filename,revision,bytes,note,uploaded_by,status,recognition_status,error_message,is_current,created_at FROM project_sources WHERE project_id=$1 ORDER BY category,filename,revision DESC LIMIT 250")
        .bind(project_id).fetch_all(&st.db).await?;
    let mut rows=String::new();
    for s in &sources {
        let id:i64=s.try_get("id")?;
        let category:String=s.try_get("category")?;
        let filename:String=s.try_get("filename")?;
        let revision:i32=s.try_get("revision")?;
        let bytes:i64=s.try_get("bytes")?;
        let status:String=s.try_get("status")?;
        let recognition:String=s.try_get("recognition_status")?;
        let error:Option<String>=s.try_get("error_message")?;
        let current:bool=s.try_get("is_current")?;
        let note:String=s.try_get("note")?;
        let state=match status.as_str(){"queued"=>"В очереди","processing"=>"Загрузка на NAS / распознавание","ready"=>"Готов","error"=>"Ошибка обработки",_=>"Неизвестно"};
        let retry=if allowed && status=="error" {format!("<form class='inline' method='post' action='/projects/{project_id}/sources/{id}/retry'>{}<button type='submit'>Повторить</button></form>",web::hidden(&st.csrf))}else{String::new()};
        rows.push_str(&format!("<tr><td><b>{}</b><br><small>{}</small></td><td>v{revision} {}</td><td>{} · {} МБ</td><td>{}<br><small>{}</small>{}</td><td><a href='/projects/{project_id}/sources/{id}/download'>Оригинал</a> {retry}</td></tr>",h(&filename),h(&note),if current{"(активная редакция)"}else{""},h(label(&category)),bytes/1_048_576,h(state),h(&recognition),error.map(|s|format!("<small class='warn'>{}</small>",h(&s))).unwrap_or_default()));
    }
    if rows.is_empty(){rows.push_str("<tr><td colspan='5'>Пока нет исходных документов. Добавьте КР, ПЗУ, ПОС, ППР или РД.</td></tr>");}
    let form=if allowed {
        let options=CATEGORIES.iter().map(|(code,title)|format!("<option value='{code}'>{}</option>",h(title))).collect::<String>();
        format!(r#"<form id='project-sources-form' method='post' action='/projects/{project_id}/sources/upload' enctype='multipart/form-data' class='project-sources-form'>
          {}<label>Раздел / назначение <select name='category' required>{options}</select></label>
          <label>Примечание к редакции <input name='note' maxlength='500' placeholder='Например: изм.3, 2026 год' style='width:min(460px,100%)'></label>
          <label>Файлы <input type='file' name='file' accept='.pdf,.doc,.docx,.xls,.xlsx,.png,.jpg,.jpeg,.tif,.tiff,.bmp,.webp,.dwg,.dxf' multiple required></label>
          <button class='primary' type='submit'>Загрузить в проект</button>
          <progress hidden max='100' value='0' style='width:240px'></progress>
          <div class='operation-feedback' role='status' aria-live='polite'></div>
        </form>"#,web::hidden(&st.csrf))
    }else{"<p class='muted'>Загружать проектные источники могут администратор ЖК и назначенные инженеры ПТО.</p>".to_owned()};
    let status_script=include_str!("../assets/project-sources.js");
    Ok(format!(r#"<section class='white-panel' id='project-sources' style='margin:22px 0;padding:22px;border-radius:14px'>
      <div class='panel-heading'><h2>Проектная документация ЖК</h2><span class='pill'>{} редакций</span></div>
      <p>Загрузите КР, ПЗУ, ПОС, ППР, РД и НТД <b>один раз на весь проект</b>. Файлы версионируются и подключаются как справочные источники ко всем созданным видам работ. Они не являются ИД подрядчика и не меняют номера поставок.</p>
      {form}
      <details open><summary><b>Файлы проекта и история редакций</b></summary>
      <table><thead><tr><th>Файл</th><th>Редакция</th><th>Раздел / размер</th><th>Состояние</th><th>Действия</th></tr></thead><tbody>{rows}</tbody></table></details>
      <small class='muted'>Если файл с тем же именем изменился, создаётся новая редакция. Неактуальные редакции остаются в истории. OCR выполняется в фоне.</small>
      <p><a class='buttonlink' href='/projects/{project_id}#project-sources'>Обновить статусы обработки</a></p>
    </section><script>{status_script}</script>"#,sources.len()))
}

fn sha256(path:&FsPath)->Result<String>{
    let mut file=std::fs::File::open(path)?;let mut hash=Sha256::new();let mut buf=[0u8;131072];
    loop {let n=file.read(&mut buf)?;if n==0{break;} hash.update(&buf[..n]);}
    Ok(hex::encode(hash.finalize()))
}

pub async fn upload(State(st):State<Shared>,Extension(actor):Extension<Principal>,Path(project_id):Path<i64>,mut multipart:Multipart)->Result<Redirect,WebError> {
    if !can_upload(&st,&actor,project_id).await{return Err(WebError::forbidden());}
    let project=sqlx::query_scalar::<_,String>("SELECT name FROM projects WHERE id=$1")
        .bind(project_id).fetch_optional(&st.db).await?.ok_or_else(WebError::not_found)?;
    let mut csrf=String::new();let mut category=String::new();let mut note=String::new();
    let mut files:Vec<(tempfile::NamedTempFile,String)>=Vec::new();let mut names=HashSet::new();
    while let Some(mut field)=multipart.next_field().await.map_err(|e|WebError::bad(e.to_string()))? {
        match field.name().unwrap_or("") {
            "csrf"=>csrf=field.text().await.map_err(|e|WebError::bad(e.to_string()))?,
            "category"=>category=field.text().await.map_err(|e|WebError::bad(e.to_string()))?,
            "note"=>note=field.text().await.map_err(|e|WebError::bad(e.to_string()))?,
            "file"=>{
                if files.len()>=20{return Err(WebError::bad("За один раз не более 20 файлов"));}
                let raw=field.file_name().unwrap_or("");
                let filename=repo::validate_segment(raw).map_err(|_|WebError::bad("Недопустимое имя файла"))?.to_string();
                if filename.starts_with("~$") || !docs::supported(FsPath::new(&filename)){
                    return Err(WebError::bad("Допускаются PDF, DOC/DOCX, XLS/XLSX, DWG/DXF и изображения"));
                }
                if !names.insert(filename.to_lowercase()){return Err(WebError::bad("Два файла с одинаковым названием"));}
                let staging=st.cfg.temp_root.join("project-sources");std::fs::create_dir_all(&staging)?;
                let mut tmp=tempfile::NamedTempFile::new_in(staging)?;let mut size=0u64;
                while let Some(chunk)=field.chunk().await.map_err(|e|WebError::bad(e.to_string()))? {
                    size=size.saturating_add(chunk.len() as u64);
                    if size>st.cfg.max_file_mb*1024*1024 {return Err(WebError::bad("Превышен MAX_FILE_MB"));}
                    tmp.write_all(&chunk)?;
                }
                if size==0{return Err(WebError::bad("Пустой файл"));}
                files.push((tmp,filename));
            },
            _=>{}
        }
    }
    web::verify(&csrf,&st)?;
    if !CATEGORIES.iter().any(|(code,_)|*code==category) {return Err(WebError::bad("Выберите раздел проектной документации"));}
    if note.len()>500 || note.chars().any(char::is_control){return Err(WebError::bad("Недопустимое примечание"));}
    if files.is_empty(){return Err(WebError::bad("Выберите проектные документы"));}
    let mut spawned=Vec::new();
    for (tmp,filename) in files {
        let digest=sha256(tmp.path()).map_err(anyhow::Error::from)?;
        let old=sqlx::query("SELECT revision,sha256,status FROM project_sources WHERE project_id=$1 AND category=$2 AND filename=$3 ORDER BY revision DESC LIMIT 1")
            .bind(project_id).bind(&category).bind(&filename).fetch_optional(&st.db).await?;
        if let Some(ref prev)=old {
            if prev.try_get::<String,_>("sha256")?==digest && prev.try_get::<String,_>("status")?=="ready" {continue;}
        }
        let revision:i32=old.map(|r|r.try_get::<i32,_>("revision").unwrap_or(0)+1).unwrap_or(1);
        let archive=st.cfg.data_root.join("project_sources").join(project_id.to_string()).join(&digest[..2]);
        std::fs::create_dir_all(&archive)?;
        let ext=FsPath::new(&filename).extension().and_then(|x|x.to_str()).unwrap_or("bin");
        let archive=archive.join(format!("{digest}.{ext}"));
        if archive.exists() {
            if sha256(&archive).map_err(anyhow::Error::from)?!=digest {
                return Err(WebError::bad("Несовпадение SHA-256 ранее сохранённого оригинала"));
            }
        } else {
            let staged=tempfile::NamedTempFile::new_in(archive.parent().unwrap())?;
            std::fs::copy(tmp.path(),staged.path())?;
            if sha256(staged.path()).map_err(anyhow::Error::from)?!=digest{return Err(WebError::bad("Файл изменён при сохранении"));}
            match staged.persist_noclobber(&archive){Ok(_)=>(),Err(e) if archive.exists()=>{let _=e;},Err(e)=>return Err(WebError::from(anyhow::Error::from(e.error)))}
        }
        let bytes=std::fs::metadata(&archive)?.len() as i64;
        let source_id:i64=sqlx::query_scalar("INSERT INTO project_sources(project_id,category,filename,revision,sha256,archive_path,bytes,note,uploaded_by,status) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,'queued') RETURNING id")
            .bind(project_id).bind(&category).bind(&filename).bind(revision).bind(&digest)
            .bind(archive.to_string_lossy().to_string()).bind(bytes).bind(&note).bind(who(&actor))
            .fetch_one(&st.db).await?;
        spawned.push(source_id);
    }
    repo::audit(&st.db,&who(&actor),"project_source_upload",&format!("project={project_id} files={} name={project}",spawned.len())).await?;
    for source_id in spawned {let worker=st.clone();tokio::spawn(async move{process_source(&worker,source_id).await;});}
    Ok(Redirect::to(&format!("/projects/{project_id}#project-sources")))
}

async fn process_source(st:&Shared,id:i64){
    let result=process_source_inner(st,id).await;
    if let Err(err)=result {
        tracing::error!(source_id=id,"Project source processing failed: {err:#}");
        let _=sqlx::query("UPDATE project_sources SET status='error',error_message=$2,processed_at=now() WHERE id=$1")
           .bind(id).bind(format!("{err:#}").chars().take(1200).collect::<String>()).execute(&st.db).await;
    }
}
async fn process_source_inner(st:&Shared,id:i64)->Result<()> {
    let source=sqlx::query("SELECT s.project_id,s.category,s.filename,s.revision,s.sha256,s.archive_path,s.nas_path,p.name FROM project_sources s JOIN projects p ON p.id=s.project_id WHERE s.id=$1")
        .bind(id).fetch_one(&st.db).await?;
    let project_id:i64=source.try_get("project_id")?;
    let category:String=source.try_get("category")?;
    let filename:String=source.try_get("filename")?;
    let revision:i32=source.try_get("revision")?;
    let archived:String=source.try_get("archive_path")?;
    let project:String=source.try_get("name")?;
    sqlx::query("UPDATE project_sources SET status='processing',error_message=NULL WHERE id=$1")
        .bind(id).execute(&st.db).await?;
    // After a restart or an OCR/sync error, an already-uploaded NAS original
    // must not be uploaded again with overwrite=false.
    let stored_nas:Option<String>=source.try_get("nas_path")?;
    let nas_file=if let Some(existing)=stored_nas {existing} else {
        let root=contractor::selected_root(st).await?;
        // Per-attempt folder avoids overwriting previous immutable originals
        // if a server crashed after NAS accepted the bytes but before commit.
        let attempt=uuid::Uuid::new_v4().simple().to_string();
        let nas_folder=format!("{root}/{project}/ПРОЕКТНАЯ ДОКУМЕНТАЦИЯ/{category}/редакция-{revision}-{id}-{}",&attempt[..8]);
        let path=format!("{nas_folder}/{filename}");
        let session=st.nas.connect().await?;
        let sent=session.upload_file(&nas_folder,&filename,FsPath::new(&archived)).await;
        session.logout().await;
        sent.context("Не удалось сохранить проектный документ на Synology")?;
        sqlx::query("UPDATE project_sources SET nas_path=$2 WHERE id=$1")
            .bind(id).bind(&path).execute(&st.db).await?;
        path
    };
    // Project references are extracted locally to avoid unintentionally sharing
    // confidential project drawings with an optional cloud OCR provider.
    let mut local_cfg=st.cfg.clone();local_cfg.ocr_mode="local".into();local_cfg.vibe_ocr_enabled=false;
    let parsed=docs::extract_with_progress(FsPath::new(&archived),local_cfg.ocr_max_pages,&local_cfg,None).await;
    let mut tx=st.db.begin().await?;
    sqlx::query("UPDATE project_sources SET is_current=false WHERE project_id=$1 AND category=$2 AND filename=$3 AND id<>$4 AND is_current")
        .bind(project_id).bind(&category).bind(&filename).bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE project_sources SET status='ready',is_current=true,nas_path=$2,recognized_text=$3,recognition_status=$4,recognition_confidence=$5,processed_at=now() WHERE id=$1")
        .bind(id).bind(&nas_file).bind(&parsed.text).bind(&parsed.status).bind(parsed.confidence).execute(&mut *tx).await?;
    tx.commit().await?;
    sync_source_to_packages(&st.db,id).await?;
    Ok(())
}

/// Creates/updates the existing package-reference document view without
/// duplicating original bytes. Check engine and AI can consume it unmodified.
async fn sync_source_to_packages(db:&PgPool,id:i64)->Result<()> {
    let src=sqlx::query("SELECT project_id,category,filename,sha256,archive_path,recognized_text,recognition_status,recognition_confidence,note,status,is_current FROM project_sources WHERE id=$1")
        .bind(id).fetch_one(db).await?;
    if src.try_get::<String,_>("status")?!="ready" || !src.try_get::<bool,_>("is_current")? {return Ok(());}
    let project_id:i64=src.try_get("project_id")?;
    let category:String=src.try_get("category")?;
    let filename:String=src.try_get("filename")?;
    let relative=doc_rel(&category,&filename);
    let sha:String=src.try_get("sha256")?;
    let archive:String=src.try_get("archive_path")?;
    let text:String=src.try_get("recognized_text")?;
    let status:String=src.try_get("recognition_status")?;
    let conf:Option<f32>=src.try_get("recognition_confidence")?;
    let note:String=src.try_get("note")?;
    let pkgs:Vec<i64>=sqlx::query_scalar("SELECT id FROM packages WHERE project_id=$1")
        .bind(project_id).fetch_all(db).await?;
    for package_id in pkgs {
        sync_to_package(db,id,package_id,&category,&relative,&sha,&archive,&text,&status,conf,&note).await?;
    }
    Ok(())
}
async fn sync_to_package(db:&PgPool,source_id:i64,package_id:i64,category:&str,relative:&str,sha:&str,archive:&str,text:&str,status:&str,confidence:Option<f32>,note:&str)->Result<()> {
    let mut tx=db.begin().await?;
    let previous=sqlx::query("SELECT id,version,sha256 FROM documents WHERE package_id=$1 AND relative_path=$2 ORDER BY version DESC LIMIT 1 FOR UPDATE")
        .bind(package_id).bind(relative).fetch_optional(&mut *tx).await?;
    if let Some(ref old)=previous {
        if old.try_get::<String,_>("sha256")?==sha {
            let doc_id:i64=old.try_get("id")?;
            sqlx::query("UPDATE documents SET is_present=true WHERE id=$1").bind(doc_id).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO project_source_package_documents(project_source_id,document_id) VALUES($1,$2) ON CONFLICT DO NOTHING")
                .bind(source_id).bind(doc_id).execute(&mut *tx).await?;
            tx.commit().await?;return Ok(());
        }
    }
    let version:i32=previous.as_ref().map(|row|row.try_get::<i32,_>("version").unwrap_or(0)+1).unwrap_or(1);
    if let Some(old)=previous {
        let old_id:i64=old.try_get("id")?;
        sqlx::query("UPDATE documents SET is_present=false WHERE id=$1").bind(old_id).execute(&mut *tx).await?;
    }
    let doc_id:i64=sqlx::query_scalar("INSERT INTO documents(package_id,relative_path,source_kind,origin,is_present,sha256,version,archive_path,document_type,catalog_code,recognized_text,recognition_status,recognition_confidence) VALUES($1,$2,'REFERENCE','upload',true,$3,$4,$5,$6,NULL,$7,$8,$9) RETURNING id")
        .bind(package_id).bind(relative).bind(sha).bind(version).bind(archive).bind(label(category)).bind(text).bind(status).bind(confidence)
        .fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO project_source_package_documents(project_source_id,document_id) VALUES($1,$2)")
        .bind(source_id).bind(doc_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO reference_materials(document_id,category,note,uploaded_by) VALUES($1,$2,$3,'project-library')")
        .bind(doc_id).bind(ref_category(category)).bind(format!("Общий проектный источник: {}. {}",label(category),note)).execute(&mut *tx).await?;
    // Design sources have changed. Re-approve the first-block applicability
    // before treating a previous acceptance as valid for the new source revision.
    sqlx::query("UPDATE packages SET block1_approved_at=NULL,block1_approved_by=NULL,block1_scope_approved_at=NULL,block1_scope_approved_by=NULL WHERE id=$1")
        .bind(package_id).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

/// Resume queued project files and jobs interrupted while processing on boot.
/// A prior DSM upload is skipped when nas_path is already stored.
pub async fn recover_interrupted(st:&Shared)->Result<()> {
    let ids:Vec<i64>=sqlx::query_scalar("UPDATE project_sources SET status='queued' WHERE status IN ('queued','processing') RETURNING id")
        .fetch_all(&st.db).await?;
    for id in ids {let worker=st.clone();tokio::spawn(async move{process_source(&worker,id).await;});}
    Ok(())
}

/// Source library already exists when a new work package is created.
pub async fn sync_project_to_package(db:&PgPool,project_id:i64,package_id:i64)->Result<()> {
    let sources=sqlx::query_scalar::<_,i64>("SELECT id FROM project_sources WHERE project_id=$1 AND status='ready' AND is_current=true")
        .bind(project_id).fetch_all(db).await?;
    for id in sources {
        let src=sqlx::query("SELECT category,filename,sha256,archive_path,recognized_text,recognition_status,recognition_confidence,note FROM project_sources WHERE id=$1")
            .bind(id).fetch_one(db).await?;
        let category:String=src.try_get("category")?;
        let file:String=src.try_get("filename")?;
        let relative=doc_rel(&category,&file);
        let sha:String=src.try_get("sha256")?;
        let archive:String=src.try_get("archive_path")?;
        let text:String=src.try_get("recognized_text")?;
        let status:String=src.try_get("recognition_status")?;
        let conf:Option<f32>=src.try_get("recognition_confidence")?;
        let note:String=src.try_get("note")?;
        sync_to_package(db,id,package_id,&category,&relative,&sha,&archive,&text,&status,conf,&note).await?;
    }
    Ok(())
}

#[derive(Deserialize)]pub struct RetryForm {pub csrf:String}
pub async fn retry(State(st):State<Shared>,Extension(actor):Extension<Principal>,Path((project_id,id)):Path<(i64,i64)>,Form(form):Form<RetryForm>)->Result<Redirect,WebError> {
    web::verify(&form.csrf,&st)?;
    if !can_upload(&st,&actor,project_id).await{return Err(WebError::forbidden());}
    let updated=sqlx::query("UPDATE project_sources SET status='queued',error_message=NULL WHERE id=$1 AND project_id=$2 AND status='error'")
        .bind(id).bind(project_id).execute(&st.db).await?;
    if updated.rows_affected()!=1{return Err(WebError::bad("Повторная обработка доступна только для записи с ошибкой"));}
    let worker=st.clone();tokio::spawn(async move{process_source(&worker,id).await;});
    Ok(Redirect::to(&format!("/projects/{project_id}#project-sources")))
}

pub async fn download(State(st):State<Shared>,Path((project_id,id)):Path<(i64,i64)>)->Result<Response,WebError> {
    let row=sqlx::query("SELECT filename,archive_path FROM project_sources WHERE id=$1 AND project_id=$2")
        .bind(id).bind(project_id).fetch_optional(&st.db).await?.ok_or_else(WebError::not_found)?;
    let filename:String=row.try_get("filename")?;
    let stored:String=row.try_get("archive_path")?;
    let path=std::fs::canonicalize(stored).map_err(|_|WebError::not_found())?;
    let allowed_root=std::fs::canonicalize(st.cfg.data_root.join("project_sources"))?;
    if !path.starts_with(&allowed_root){return Err(WebError::forbidden());}
    let file=tokio::fs::File::open(path).await?;
    let stream=tokio_util::io::ReaderStream::new(file);
    // RFC 5987 preserves Cyrillic filenames; ASCII fallback for legacy browsers.
    let encoded=url::form_urlencoded::byte_serialize(filename.as_bytes()).collect::<String>();
    let content_disposition=format!("attachment; filename=\"project-source-{id}\"; filename*=UTF-8''{encoded}");
    Ok(Response::builder().header(header::CONTENT_TYPE,"application/octet-stream")
        .header(header::CONTENT_DISPOSITION,content_disposition)
        .header(header::CACHE_CONTROL,"no-store")
        .body(Body::from_stream(stream))?)
}

#[cfg(test)]
mod tests {
  use super::*;
  #[test] fn project_types_and_paths() {
    assert_eq!(doc_rel("KR","КР.pdf"),"REFERENCE/PD/PROJECT/KR/КР.pdf");
    assert_eq!(doc_rel("RD","РД.pdf"),"REFERENCE/RD/PROJECT/RD/РД.pdf");
    assert_eq!(doc_rel("NTD","СП.pdf"),"REFERENCE/NTD/PROJECT/NTD/СП.pdf");
    assert_eq!(ref_category("PPR"),"PD");
  }
}
