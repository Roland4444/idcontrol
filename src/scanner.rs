use crate::{catalog::Catalog, config::Config, docs, engine, repo, synology::SynologyClient};
use anyhow::{bail, Context, Result};
use sha2::{Digest,Sha256};
use sqlx::PgPool;
use std::{collections::HashSet,fs::{self,File},io::Read,path::Path,sync::Arc,time::{SystemTime,UNIX_EPOCH}};
use tracing::{info,warn};

fn source_details(root:&Path,path:&Path)->Option<(String,String,String,String)>{
    let comps:Vec<String>=path.strip_prefix(root).ok()?.components()
        .map(|c|c.as_os_str().to_string_lossy().into_owned()).collect();
    if comps.len()<4{return None;}
    let kind=match comps[2].to_lowercase().as_str(){
        "id"|"ид"|"исполнительная документация"=>"ID",
        "reference"|"эталон"|"пд-рд"|"пд_рд"|"pd_rd"|"пд"|"рд"=>"REFERENCE",
        _=>return None
    };
    if comps.iter().any(|c|repo::validate_segment(c).is_err()) {return None;}
    Some((comps[0].clone(),comps[1].clone(),kind.into(),comps[2..].join("/")))
}
/// File Station supports three layouts under SYNOLOGY_ROOT:
/// - ИД/... and ПД/...                          (one project, one default package)
/// - Проект/ИД/... and Проект/ПД/...            (project, default package)
/// - Проект/Вид работ/ИД/... and .../ПД/...     (full hierarchy)
/// An unambiguous virtual layout lets the existing ingestion pipeline and database
/// always work with the canonical Project/Package/ИД|ПД-РД/File shape.
#[derive(Debug, PartialEq, Eq)]
struct NormalizedNasFile {
    project: String,
    work: String,
    db_relative: String,
    mirror_relative: String,
}
fn canonical_kind(name:&str)->Option<&'static str> {
    match name.to_lowercase().as_str() {
        "ид" | "id" | "исполнительная документация" => Some("ИД"),
        "пд" | "рд" | "пд-рд" | "пд_рд" | "pd_rd" | "reference" | "эталон" => Some("ПД-РД"),
        _ => None,
    }
}
fn normalize_remote_file(rel:&str, default_project:&str, default_work:&str)->Option<NormalizedNasFile>{
    let parts:Vec<&str>=rel.split('/').collect();
    if parts.iter().any(|part|repo::validate_segment(part).is_err()){return None;}
    let (project,work,kind,tail):(&str,&str,&str,&[&str])=
        if parts.len()>=2 {
            if let Some(kind)=canonical_kind(parts[0]) {
                (default_project,default_work,kind,&parts[1..])
            } else if parts.len()>=3 {
                if let Some(kind)=canonical_kind(parts[1]) {
                    (parts[0],default_work,kind,&parts[2..])
                } else if parts.len()>=4 {
                    (parts[0],parts[1],canonical_kind(parts[2])?,&parts[3..])
                } else {return None}
            } else {return None}
        } else {return None};
    if tail.is_empty() || tail.iter().any(|p|p.is_empty()) {return None;}
    let db_relative=format!("{kind}/{}",tail.join("/"));
    let mirror_relative=format!("{project}/{work}/{db_relative}");
    Some(NormalizedNasFile{project:project.into(),work:work.into(),db_relative,mirror_relative})
}

fn sha256(path:&Path)->Result<String>{
    let mut hasher=Sha256::new();let mut file=File::open(path)?;let mut buf=[0;1024*1024];
    loop{let n=file.read(&mut buf)?;if n==0{break;}hasher.update(&buf[..n]);}
    Ok(hex::encode(hasher.finalize()))
}
fn relative_remote_path<'a>(root:&str, full:&'a str)->Option<&'a str>{
    full.strip_prefix(root)?.strip_prefix('/')
}
pub async fn ingest(db:&PgPool,cfg:&Config,catalog:&Catalog,root:&Path,path:&Path,origin:&str)->Result<Option<i64>>{
    let Some((project,work,kind,relative))=source_details(root,path) else {return Ok(None)};
    let pr=repo::get_or_create_project(db,&project).await?;
    let mut pkg=repo::get_or_create_package(db,pr.id,&work).await?;
    if pkg.catalog_name.is_none() {
        if let Some(w)=catalog.closest(&work){
            sqlx::query("UPDATE packages SET catalog_name=$1 WHERE id=$2 AND catalog_name IS NULL")
                .bind(&w.name).bind(pkg.id).execute(db).await?;
            pkg.catalog_name=Some(w.name.clone());
        }
    }
    let src=path.to_owned();
    let digest=tokio::task::spawn_blocking(move||sha256(&src)).await??;
    let latest=repo::latest_document(db,pkg.id,&relative).await?;
    if let Some(ref prev)=latest {
        if prev.sha256==digest {
            if prev.is_present {return Ok(None);}
            sqlx::query("UPDATE documents SET is_present=true WHERE id=$1").bind(prev.id).execute(db).await?;
            return Ok(Some(pkg.id));
        }
    }
    let version=latest.as_ref().map_or(1,|d|d.version+1);
    let suffix=path.extension().and_then(|e|e.to_str()).unwrap_or("bin").to_lowercase();
    let archive=cfg.data_root.join("originals").join(pr.id.to_string()).join(pkg.id.to_string())
        .join(&digest[..2]).join(format!("{digest}.{suffix}"));
    let src=path.to_owned();let dst=archive.clone();let digest_again=digest.clone();
    tokio::task::spawn_blocking(move||->Result<()>{
        fs::create_dir_all(dst.parent().unwrap())?;
        if dst.exists(){
            if sha256(&dst)?!=digest_again{bail!("archive checksum collision/corruption");}
        }else{
            let tmp=tempfile::NamedTempFile::new_in(dst.parent().unwrap())?;
            fs::copy(&src,tmp.path())?;
            if sha256(tmp.path())?!=digest_again {bail!("file changed while reading: {}",src.display());}
            match tmp.persist_noclobber(&dst){Ok(_)=>{},Err(e) if dst.exists()=>{let _=e;},Err(e)=>return Err(e.error.into())}
        }
        Ok(())
    }).await??;
    let stored=archive.clone();
    info!(file=%relative,"New/updated file: OCR processing started");
    let rel_for_log=relative.clone();
    let callback:docs::ProgressCallback=Arc::new(move |event| {
        info!(file=%rel_for_log,stage=event.stage,current=event.current,total=event.total,"{}",event.message);
    });
    let mut safe_cfg=cfg.clone();
    if kind=="REFERENCE" { safe_cfg.ocr_mode="local".into(); safe_cfg.vibe_ocr_enabled=false; }
    let parsed=docs::extract_with_progress(&stored,cfg.ocr_max_pages,&safe_cfg,Some(callback)).await;
    info!(file=%relative,status=%parsed.status,"New/updated file: OCR processing finished");
    let classified=if kind=="ID"{
        catalog.classify(path.file_name().unwrap_or_default().to_str().unwrap_or(""),&parsed.text,
            pkg.catalog_name.as_deref().and_then(|n|catalog.work_type(n)))
    }else{None};
    let (code,dtype)=classified.map(|(a,b)|(Some(a),Some(b)))
        .unwrap_or_else(||(None,if kind=="REFERENCE"{Some("Эталон ПД/РД".to_string())}
            else {crate::catalog::guess_general_document_type(path.file_name().unwrap_or_default().to_str().unwrap_or(""),&parsed.text)}));
    let mut tx=db.begin().await?;
    if let Some(old)=&latest {
        sqlx::query("UPDATE documents SET is_present=false WHERE id=$1")
            .bind(old.id).execute(&mut *tx).await?;
    }
    let rec:(i64,)=sqlx::query_as("INSERT INTO documents(package_id,relative_path,source_kind,origin,sha256,version,archive_path,document_type,catalog_code,recognized_text,recognition_status,recognition_confidence) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING id")
        .bind(pkg.id).bind(&relative).bind(&kind).bind(origin).bind(&digest).bind(version)
        .bind(archive.to_string_lossy().to_string()).bind(dtype).bind(code)
        .bind(parsed.text).bind(parsed.status).bind(parsed.confidence).fetch_one(&mut *tx).await?;
    tx.commit().await?;
    if let Err(err)=crate::classification::maybe_auto_enqueue(db,cfg,rec.0).await {
        warn!(file=%relative,"AI auto classification was not queued: {err:#}");
    }
    repo::audit(db,origin,"file_ingested",&format!("{} sha256={} version={}",path.display(),digest,version)).await?;
    Ok(Some(pkg.id))
}

#[derive(serde::Serialize)]
pub struct ScanResult {pub synology_connected:bool,pub new_documents:usize,pub affected_packages:usize}

/// A complete successful remote traversal is required before disappearance reconciliation.
/// Errors in listing, paging, login or decoding never mark documents deleted.
pub async fn scan_once(db:&PgPool,cfg:&Config,catalog:&Catalog,client:&SynologyClient)->Result<ScanResult>{
    // DSM may invalidate a File Station SID between login and a directory listing
    // (API codes 106=timeout, 107=interrupted, 119=invalid session).
    // Restart the entire traversal with a fresh SID, never continuing from
    // a partially traversed directory tree or marking unseen files as removed.
    for attempt in 1..=3 {
        let session=client.connect().await
            .with_context(||format!("DSM login/discovery failed on scan attempt {attempt}"))?;
        let result=scan_session(db,cfg,catalog,&session).await;
        session.logout().await;
        match result {
            Ok(scan) => {
                if attempt>1 {info!("Synology scan recovered after DSM session renewal (attempt {attempt})");}
                return Ok(scan);
            }
            Err(ref error) if attempt<3 && error
                .downcast_ref::<crate::synology::DsmListError>()
                .is_some_and(|dsm| matches!(dsm.code,106|107|119)) => {
                    let code=error.downcast_ref::<crate::synology::DsmListError>()
                        .map(|e|e.code).unwrap_or_default();
                    warn!("DSM File Station session invalid (code {code}); new login and full rescan, attempt {}/3",attempt+1);
                    tokio::time::sleep(std::time::Duration::from_millis(600*attempt as u64)).await;
                }
            Err(error) => return Err(error),
        }
    }
    unreachable!("all scan attempts returned or continued")
}
async fn scan_session(
    db:&PgPool,cfg:&Config,catalog:&Catalog,session:&crate::synology::SynologySession<'_>
)->Result<ScanResult>{
    let entries=session.list_files().await?;
    let files_reported=entries.len();
    let mut seen=HashSet::new();let mut affected=HashSet::new();let mut ingested=0;
    let mut supported=0usize;
    let mut ignored_layout=0usize;
    let mut ignored_extension=0usize;
    for e in entries {
        let Some(rel)=relative_remote_path(&cfg.synology_root,&e.path) else {ignored_layout+=1;continue};
        if !crate::docs::supported(Path::new(rel)) ||
           rel.split('/').last().is_some_and(|n|n.starts_with("~$") || n.starts_with('.')) {
            ignored_extension+=1;
            continue;
        }
        let Some(layout)=normalize_remote_file(rel,&cfg.synology_project_name,&cfg.synology_default_work) else {
            ignored_layout+=1;
            continue;
        };
        supported+=1;
        seen.insert((layout.project.clone(),layout.work.clone(),layout.db_relative.clone()));
        if e.size>cfg.max_file_mb*1024*1024 {warn!("Ignoring oversized NAS file: {}",e.path);continue;}
        let now=SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs() as i64;
        if e.mtime>now || now-e.mtime<cfg.stable_seconds as i64 {continue;}
        // Avoid repeatedly downloading unchanged files. If an archived document is missing
        // from the active set, do not trust an old metadata signature.
        let meta=sqlx::query_as::<_,(i64,i64)>("SELECT size_bytes,modified_epoch FROM synology_file_index WHERE remote_path=$1")
            .bind(&e.path).fetch_optional(db).await?;
        let pr=repo::get_or_create_project(db,&layout.project).await?;
        let pkg=repo::get_or_create_package(db,pr.id,&layout.work).await?;
        let latest=repo::latest_document(db,pkg.id,&layout.db_relative).await?;
        if meta.is_some_and(|(s,m)|s==e.size as i64 && m==e.mtime) &&
           latest.is_some_and(|d|d.is_present && d.origin=="synology") {continue;}
        // Isolated tempdir prevents read-only NAS files from being modified and leaves no
        // half-completed temporary scans after the current operation finishes.
        let tmp=tempfile::tempdir_in(&cfg.temp_root)?;
        let dest=tmp.path().join(&layout.mirror_relative);
        fs::create_dir_all(dest.parent().context("Bad relative path")?)?;
        info!(file=%e.path,bytes=e.size,"Synology: downloading new/changed file");
        if let Err(err)=session.download_to(&e.path,&dest,cfg.max_file_mb*1024*1024).await {
            warn!("Cannot download {}: {err:#}",e.path);
            continue;
        }
        let got=fs::metadata(&dest)?.len();
        if got!=e.size {warn!("Size changed during Synology download: {} expected={} actual={}",e.path,e.size,got);continue;}
        match ingest(db,cfg,catalog,tmp.path(),&dest,"synology").await {
            Ok(updated)=>{
                if let Some(id)=updated {affected.insert(id);ingested+=1;}
                sqlx::query("INSERT INTO synology_file_index(remote_path,size_bytes,modified_epoch) VALUES ($1,$2,$3) ON CONFLICT(remote_path) DO UPDATE SET size_bytes=EXCLUDED.size_bytes,modified_epoch=EXCLUDED.modified_epoch,scanned_at=now()")
                    .bind(&e.path).bind(e.size as i64).bind(e.mtime).execute(db).await?;
            },
            Err(err)=>warn!("Failed to analyze Synology file {}: {err:#}",e.path)
        }
    }
    // Never interpret an empty NAS folder or unsupported folder layout as deletion.
    let allow_removals=files_reported>0 && supported>0;
    if files_reported==0 {warn!("No files reported by DSM: bulk disappearance reconciliation skipped");}
    info!("DSM scan detail: {files_reported} files returned, {supported} matched layout/extension, {ignored_layout} skipped by folder layout, {ignored_extension} skipped by extension/temp filename");
    if files_reported>0 && supported==0 {
        warn!("DSM found files but none matched supported folder structures; check SYNOLOGY_ROOT and the ПД/ИД hierarchy");
    }
    if allow_removals {
        let all=sqlx::query_as::<_,(i64,String,String,String,bool)>("SELECT d.id,p.name,k.name,d.relative_path,d.is_present FROM documents d JOIN packages k ON k.id=d.package_id JOIN projects p ON p.id=k.project_id WHERE d.origin='synology' AND d.id=(SELECT max(d2.id) FROM documents d2 WHERE d2.package_id=d.package_id AND d2.relative_path=d.relative_path) ORDER BY d.id DESC")
            .fetch_all(db).await?;
        for (id,pr,work,rel,present) in all {
            if present && !seen.contains(&(pr.clone(),work.clone(),rel.clone())) {
                let pkgid:(i64,)=sqlx::query_as("UPDATE documents SET is_present=false WHERE id=$1 RETURNING package_id")
                    .bind(id).fetch_one(db).await?;
                affected.insert(pkgid.0);
                repo::audit(db,"scanner","nas_file_missing",&format!("{pr}/{work}/{rel}")).await?;
            }
        }
    }
    for id in &affected {engine::analyze(db,catalog,*id,"synology").await?;}
    info!("Synology scan: {ingested} new/updated docs, {} affected packages",affected.len());
    Ok(ScanResult{synology_connected:true,new_documents:ingested,affected_packages:affected.len()})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn paths() {
        let root=Path::new("/nas");
        let d=source_details(root,Path::new("/nas/ЖК/Земляные работы/ИД/акт.docx")).unwrap();
        assert_eq!(d.2,"ID"); assert_eq!(d.3,"ИД/акт.docx");
        assert!(source_details(root,Path::new("/nas/ЖК/непонятно/акт.docx")).is_none());
        assert!(source_details(root,Path::new("/nas/ЖК/Вид/ИД/../escape.pdf")).is_none());
    }
    #[test] fn accepts_flat_pd_id_and_subfolders() {
        let a=normalize_remote_file("ПД/проект.pdf", "тест (esinitsin)", "Вид работ не указан").unwrap();
        assert_eq!(a.project,"тест (esinitsin)");
        assert_eq!(a.work,"Вид работ не указан");
        assert_eq!(a.db_relative,"ПД-РД/проект.pdf");
        assert_eq!(a.mirror_relative,"тест (esinitsin)/Вид работ не указан/ПД-РД/проект.pdf");
        let b=normalize_remote_file("ИД/акты/акт.docx", "Тест", "Вид работ не указан").unwrap();
        assert_eq!(b.db_relative,"ИД/акты/акт.docx");
    }
    #[test] fn accepts_project_and_work_folders() {
        let a=normalize_remote_file("ЖК Северный/ИД/акт.docx", "Ignored", "Общие работы").unwrap();
        assert_eq!(a.project,"ЖК Северный");
        assert_eq!(a.work,"Общие работы");
        let b=normalize_remote_file("ЖК Северный/Земляные работы/ПД/rd.pdf", "Ignored", "Ignored").unwrap();
        assert_eq!(b.db_relative,"ПД-РД/rd.pdf");
        assert_eq!(b.work,"Земляные работы");
        assert!(normalize_remote_file("неизвестная/акт.pdf","X","Y").is_none());
        assert!(normalize_remote_file("ИД/../act.docx","X","Y").is_none());
    }
    #[test] fn remote_prefix(){
        assert_eq!(relative_remote_path("/resdata/ИД","/resdata/ИД/ЖК/Работы/ИД/а.pdf"),Some("ЖК/Работы/ИД/а.pdf"));
        assert_eq!(relative_remote_path("/resdata/ИД","/resdata/ИД2/x"),None);
    }
}
