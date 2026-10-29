use crate::{catalog::Catalog, config::Config, docs, repo};
use anyhow::{bail, Result};
use sha2::{Digest,Sha256};
use sqlx::PgPool;
use std::{fs::{self,File},io::Read,path::Path,sync::Arc};
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
fn sha256(path:&Path)->Result<String>{
    let mut hasher=Sha256::new();let mut file=File::open(path)?;let mut buf=[0;1024*1024];
    loop{let n=file.read(&mut buf)?;if n==0{break;}hasher.update(&buf[..n]);}
    Ok(hex::encode(hasher.finalize()))
}
pub async fn ingest(db:&PgPool,cfg:&Config,catalog:&Catalog,root:&Path,path:&Path,origin:&str)->Result<Option<i64>>{
    let Some((project,work_folder,kind,relative))=source_details(root,path) else {return Ok(None)};
    // NAS paths cannot contain '/' inside a single folder name. Restore the
    // canonical catalog work type before assigning files to a project package.
    let work=catalog.work_types.iter().find(|w|crate::contractor::nas_work_folder(&w.name)==work_folder)
        .map(|w|w.name.as_str()).unwrap_or(&work_folder);
    let pr=repo::get_or_create_project(db,&project).await?;
    let pkg=repo::get_or_create_package(db,pr.id,work).await?;
    ingest_for_package(db,cfg,catalog,pkg,kind,relative,path,origin,None).await
}

/// Files delivered by a contractor have an authoritative package ID from the
/// one-time link. Never re-resolve this ID from folder names or NAS mappings.
pub async fn ingest_published(
    db:&PgPool,cfg:&Config,catalog:&Catalog,package_id:i64,
    path:&Path,db_relative:&str
)->Result<Option<i64>> {
    ingest_published_with_progress(db,cfg,catalog,package_id,path,db_relative,None).await
}

pub async fn ingest_published_with_progress(
    db:&PgPool,cfg:&Config,catalog:&Catalog,package_id:i64,
    path:&Path,db_relative:&str,external_progress:Option<docs::ProgressCallback>
)->Result<Option<i64>> {
    let segments:Vec<&str>=db_relative.split('/').collect();
    if segments.len()<3 || segments[0]!="ИД" || !crate::contractor::is_published_delivery_folder(segments[1]) ||
        segments.iter().any(|name|repo::validate_segment(name).is_err()) {
        bail!("Invalid published delivery relative path");
    }
    let pkg=repo::package(db,package_id).await?;
    ingest_for_package(db,cfg,catalog,pkg,"ID".into(),db_relative.to_owned(),path,"synology",external_progress).await
}

async fn ingest_for_package(
    db:&PgPool,cfg:&Config,catalog:&Catalog,mut pkg:crate::models::Package,
    kind:String,relative:String,path:&Path,origin:&str,
    external_progress:Option<docs::ProgressCallback>
)->Result<Option<i64>> {
    if pkg.catalog_name.is_none() {
        if let Some(w)=catalog.closest(&pkg.name){
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
    let archive=cfg.data_root.join("originals").join(pkg.project_id.to_string()).join(pkg.id.to_string())
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
        if let Some(callback)=&external_progress {callback(event);}
    });
    let mut safe_cfg=cfg.clone();
    if kind=="REFERENCE" { safe_cfg.ocr_mode="local".into(); safe_cfg.vibe_ocr_enabled=false; }
    let parsed=docs::extract_with_progress(&stored,cfg.ocr_max_pages,&safe_cfg,Some(callback)).await;
    info!(file=%relative,status=%parsed.status,"New/updated file: OCR processing finished");
    let classified=if kind=="ID"{
        catalog.classify(path.file_name().unwrap_or_default().to_str().unwrap_or(""),&parsed.text,
            pkg.catalog_name.as_deref().and_then(|n|catalog.work_type(n)))
    }else{None};
    let reference_type=if kind=="REFERENCE" {
        let section=relative.strip_prefix("REFERENCE/").and_then(|r|r.split('/').next());
        Some(match section {
            Some("PD")=>"Проектная документация",
            Some("RD")=>"Рабочая документация",
            Some("NTD")=>"Нормативные документы",
            Some("ORDERS")=>"Приказы и полномочия",
            Some("SRO")=>"СРО и организации",
            Some("PERMIT")=>"Разрешение на строительство",
            Some("QUALITY")=>"Документы качества материалов",
            Some("JOURNALS")=>"Журналы работ",
            Some("OTHER")=>"Прочие документы-основания",
            _=>"Справочный документ ПД/РД",
        }.to_owned())
    }else{None};
    let (code,dtype)=classified.map(|(a,b)|(Some(a),Some(b)))
        .unwrap_or_else(||(None,if kind=="REFERENCE"{reference_type}
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
}
