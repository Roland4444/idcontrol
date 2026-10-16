//! Re-run OCR/classification against the immutable local archives without NAS re-download.
//! Does NOT create a new document version for the same SHA-256.
use crate::{catalog::Catalog,config::Config,docs,engine};
use anyhow::Result;
use sqlx::PgPool;
use std::{collections::HashSet,path::Path,sync::Arc,time::Instant};
use tracing::{info,warn};

pub async fn run(db:&PgPool,cfg:&Config,catalog:&Catalog,all:bool)->Result<()> {
    let rows:Vec<(i64,i64,String,String,String,Option<String>)>=sqlx::query_as(
        "SELECT d.id,d.package_id,d.archive_path,d.relative_path,d.source_kind,k.catalog_name \
         FROM documents d JOIN packages k ON k.id=d.package_id \
         WHERE d.is_present=true \
           AND ($1 OR d.recognition_status LIKE 'error:%' OR d.recognition_status IN ('manual','partial')) \
         ORDER BY d.id")
        .bind(all).fetch_all(db).await?;
    println!("Documents to reprocess: {}",rows.len());
    let mut packages=HashSet::new();
    let mut success=0usize;
    let total=rows.len();
    for (index,(id,pkg_id,archive,relative,kind,work)) in rows.into_iter().enumerate() {
        if !Path::new(&archive).is_file() {
            eprintln!("[{}/{}] #{}: archived original missing: {}",index+1,total,id,archive);
            warn!(document_id=id,"Archived original not found, skipping");
            continue;
        }
        println!("\n[{}/{}] Document #{}: {}",index+1, total, id, relative);
        let started=Instant::now();
        let callback:docs::ProgressCallback=Arc::new(move |event| {
            println!("  [{}] {}/{} — {}",event.stage,event.current,event.total,event.message);
        });
        let parsed=docs::extract_with_progress(Path::new(&archive),cfg.ocr_max_pages,cfg,Some(callback)).await;
        println!("  Finished after {:.1}s; status={}",started.elapsed().as_secs_f64(),parsed.status);
        let file_name=Path::new(&relative).file_name().and_then(|s|s.to_str()).unwrap_or("");
        let classified=if kind=="ID" {
            catalog.classify(file_name,&parsed.text,work.as_deref().and_then(|x|catalog.work_type(x)))
        }else{None};
        let (code,dtype)=classified.map(|(c,t)|(Some(c),Some(t)))
            .unwrap_or_else(||(None,if kind=="ID" {
                crate::catalog::guess_general_document_type(file_name,&parsed.text)
            }else{None}));
        let is_error=parsed.status.starts_with("error:");
        let status=parsed.status.clone();
        sqlx::query("UPDATE documents SET recognized_text=CASE WHEN $2 LIKE 'error:%' OR btrim($1)='' THEN recognized_text ELSE $1 END,recognition_status=$2,recognition_confidence=$3,\
              document_type=COALESCE(document_type,$4),catalog_code=COALESCE(catalog_code,$5) WHERE id=$6")
            .bind(parsed.text).bind(&status).bind(parsed.confidence)
            .bind(dtype).bind(code).bind(id).execute(db).await?;
        packages.insert(pkg_id);
        if is_error {warn!(document_id=id,"OCR retry failed: {status}");} else {success+=1;}
        info!(document_id=id,recognition_status=%status,"Reprocessed document");
    }
    for pkg in &packages {engine::analyze(db,catalog,*pkg,"ocr_reprocess").await?;}
    println!("Reprocessed successfully: {success}; affected packages: {}",packages.len());
    Ok(())
}

/// Re-label existing scanned documents from their stored OCR text without any
/// VibeCode calls. Never overwrite a type selected manually by an engineer.
pub async fn classify_existing(db:&PgPool,catalog:&Catalog)->Result<()> {
    let rows:Vec<(i64,String,String,String,Option<String>)>=sqlx::query_as(
        "SELECT d.id,d.relative_path,d.source_kind,d.recognized_text,k.catalog_name \
         FROM documents d JOIN packages k ON k.id=d.package_id \
         WHERE d.is_present=true AND d.document_type IS NULL ORDER BY d.id")
        .fetch_all(db).await?;
    let mut updated=0;
    for (id,relative,kind,recognized,work) in rows {
        if kind!="ID" {continue;}
        let file_name=Path::new(&relative).file_name().and_then(|p|p.to_str()).unwrap_or("");
        let classified=work.as_deref().and_then(|name|catalog.work_type(name))
            .and_then(|wt|catalog.classify(file_name,&recognized,Some(wt)));
        let (code,dtype)=classified.map(|(c,t)|(Some(c),Some(t)))
            .unwrap_or_else(||(None,crate::catalog::guess_general_document_type(file_name,&recognized)));
        if dtype.is_none(){continue;}
        sqlx::query("UPDATE documents SET document_type=COALESCE(document_type,$1),catalog_code=COALESCE(catalog_code,$2) WHERE id=$3")
            .bind(dtype).bind(code).bind(id).execute(db).await?;
        updated+=1;
    }
    println!("Document labels updated: {updated}; no OCR requests sent");
    Ok(())
}
