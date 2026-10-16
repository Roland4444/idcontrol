//! DB-backed OCR queue, single claim per document, persisted progress.
//! Only non-sensitive metadata is logged: no key, document text, or image bytes.
use crate::{catalog::Catalog, config::Config, docs, engine, repo};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime,Utc};
use serde::Serialize;
use sqlx::{FromRow,PgPool};
use std::{path::Path, sync::Arc};
use tokio::sync::mpsc;
use tracing::{error,info,warn};

#[derive(Debug,Clone,FromRow,Serialize)]
pub struct OcrJob {
    pub id:i64,
    pub document_id:i64,
    pub package_id:i64,
    pub status:String,
    pub stage:String,
    pub current_step:i32,
    pub total_steps:i32,
    pub message:String,
    pub error_message:String,
    pub requested_by:String,
    pub created_at:DateTime<Utc>,
    pub started_at:Option<DateTime<Utc>>,
    pub updated_at:DateTime<Utc>,
    pub finished_at:Option<DateTime<Utc>>,
}
#[derive(Debug,Clone,FromRow,Serialize)]
pub struct OcrEvent {
    pub id:i64,
    pub job_id:i64,
    pub created_at:DateTime<Utc>,
    pub stage:String,
    pub current_step:i32,
    pub total_steps:i32,
    pub message:String,
}
const JOB_COLUMNS:&str="id,document_id,package_id,status,stage,current_step,total_steps,message,error_message,requested_by,created_at,started_at,updated_at,finished_at";

pub fn needs_retry(status:&str)->bool {
    status.starts_with("error:") || matches!(status,"manual"|"partial")
}

pub async fn enqueue_document(db:&PgPool,id:i64,actor:&str)->Result<Option<i64>>{
    // Only current versions of existing documents are reprocessed.
    let doc:(i64,i64)=sqlx::query_as(
        "SELECT d.id,d.package_id FROM documents d WHERE d.id=$1 AND d.is_present=true
         AND NOT EXISTS (SELECT 1 FROM documents newer WHERE newer.package_id=d.package_id
             AND newer.relative_path=d.relative_path AND newer.version>d.version)")
        .bind(id).fetch_optional(db).await?
        .ok_or_else(||anyhow!("Document not found, absent, or superseded by newer version"))?;
    let row:Option<(i64,)>=sqlx::query_as(
        "INSERT INTO ocr_jobs(document_id,package_id,requested_by) VALUES ($1,$2,$3)
         ON CONFLICT (document_id) WHERE status IN ('pending','running') DO NOTHING RETURNING id")
        .bind(doc.0).bind(doc.1).bind(actor).fetch_optional(db).await?;
    if let Some((job_id,))=row {
        write_event(db,job_id,"queued",0,0,"Задание поставлено в очередь").await?;
        info!(job_id,document_id=id,"OCR job queued");
    }
    Ok(row.map(|r|r.0))
}

pub async fn enqueue_package(db:&PgPool,package_id:i64,all:bool,actor:&str)->Result<usize>{
    let ids:Vec<(i64,)>=sqlx::query_as(
        "SELECT d.id FROM documents d WHERE d.package_id=$1 AND d.is_present=true
         AND NOT EXISTS (SELECT 1 FROM documents newer WHERE newer.package_id=d.package_id
             AND newer.relative_path=d.relative_path AND newer.version>d.version)
         AND ($2 OR d.recognition_status LIKE 'error:%' OR d.recognition_status IN ('manual','partial'))
         ORDER BY d.id")
        .bind(package_id).bind(all).fetch_all(db).await?;
    let mut inserted=0;
    for (id,) in ids {
        if enqueue_document(db,id,actor).await?.is_some() {inserted+=1;}
    }
    Ok(inserted)
}

pub async fn jobs(db:&PgPool,package_id:Option<i64>,limit:i64)->Result<Vec<OcrJob>>{
    // Fixed SQL means no concatenated user-supplied column names.
    let sql=format!("SELECT {JOB_COLUMNS} FROM ocr_jobs WHERE ($1::bigint IS NULL OR package_id=$1) ORDER BY CASE status WHEN 'running' THEN 0 WHEN 'pending' THEN 1 ELSE 2 END, id DESC LIMIT $2");
    Ok(sqlx::query_as::<_,OcrJob>(&sql).bind(package_id).bind(limit.clamp(1,100)).fetch_all(db).await?)
}

pub async fn recent_events(db:&PgPool,job_id:i64,limit:i64)->Result<Vec<OcrEvent>>{
    Ok(sqlx::query_as::<_,OcrEvent>(
        "SELECT id,job_id,created_at,stage,current_step,total_steps,message
         FROM ocr_job_events WHERE job_id=$1 ORDER BY id DESC LIMIT $2")
        .bind(job_id).bind(limit.clamp(1,100)).fetch_all(db).await?)
}

pub async fn write_event(db:&PgPool,id:i64,stage:&str,current:i32,total:i32,message:&str)->Result<()>{
    let summary=message.chars().take(320).collect::<String>();
    sqlx::query("UPDATE ocr_jobs SET stage=$2,current_step=GREATEST(current_step,$3),total_steps=GREATEST(total_steps,$4),message=$5,updated_at=now() WHERE id=$1")
        .bind(id).bind(stage).bind(current).bind(total).bind(&summary).execute(db).await?;
    sqlx::query("INSERT INTO ocr_job_events(job_id,stage,current_step,total_steps,message) VALUES ($1,$2,$3,$4,$5)")
        .bind(id).bind(stage).bind(current).bind(total).bind(&summary).execute(db).await?;
    Ok(())
}

pub async fn cancel_pending(db:&PgPool,id:i64)->Result<bool>{
    let result=sqlx::query(
        "UPDATE ocr_jobs SET status='cancelled',stage='cancelled',message='Отменено пользователем',
         updated_at=now(),finished_at=now() WHERE id=$1 AND status='pending'")
        .bind(id).execute(db).await?;
    if result.rows_affected()>0 {
        write_event(db,id,"cancelled",0,0,"Отменено до начала обработки").await?;
    }
    Ok(result.rows_affected()>0)
}

async fn claim_next(db:&PgPool)->Result<Option<OcrJob>>{
    let sql=format!("UPDATE ocr_jobs SET status='running',stage='starting',started_at=now(),updated_at=now(),
        message='Начало обработки' WHERE id=(
        SELECT id FROM ocr_jobs WHERE status='pending' ORDER BY id FOR UPDATE SKIP LOCKED LIMIT 1
        ) RETURNING {JOB_COLUMNS}");
    Ok(sqlx::query_as::<_,OcrJob>(&sql).fetch_optional(db).await?)
}

async fn process(db:&PgPool,cfg:&Config,catalog:&Catalog,job:&OcrJob)->Result<String>{
    let doc=repo::document(db,job.document_id).await?;
    if doc.package_id!=job.package_id {return Err(anyhow!("Document/package mismatch"));}
    if !doc.is_present || !Path::new(&doc.archive_path).is_file() {
        return Err(anyhow!("Archived document is absent: {}",doc.relative_path));
    }
    let (sender,mut receiver)=mpsc::unbounded_channel::<docs::OcrProgress>();
    let callback:docs::ProgressCallback=Arc::new(move |event| {
        let _=sender.send(event);
    });
    let db_events=db.clone();
    let job_id=job.id;
    let doc_id=job.document_id;
    // Progress consumer runs while the OCR future is busy with HTTP or PDF rendering.
    let progress_writer=tokio::spawn(async move {
        while let Some(event)=receiver.recv().await {
            info!(job_id,document_id=doc_id,stage=event.stage,current=event.current,total=event.total,"{}",event.message);
            if let Err(err)=write_event(&db_events,job_id,event.stage,event.current,event.total,&event.message).await {
                warn!(job_id,"Could not persist OCR progress: {err:#}");
            }
        }
    });
    let mut safe_cfg=cfg.clone();
    if doc.source_kind=="REFERENCE" { safe_cfg.ocr_mode="local".into(); safe_cfg.vibe_ocr_enabled=false; }
    let extraction=docs::extract_with_progress(Path::new(&doc.archive_path),cfg.ocr_max_pages,&safe_cfg,Some(callback)).await;
    // Ensure all progress events are persisted before final job state.
    if let Err(err)=progress_writer.await {warn!(job_id=job.id,"Progress writer stopped: {err}");}
    write_event(db,job.id,"saving",0,0,"Сохранение распознанного текста в PostgreSQL").await?;
    let filename=Path::new(&doc.relative_path).file_name().and_then(|s|s.to_str()).unwrap_or("");
    let pkg=repo::package(db,doc.package_id).await?;
    let classification=if doc.source_kind=="ID" {
        catalog.classify(filename,&extraction.text,pkg.catalog_name.as_deref().and_then(|x|catalog.work_type(x)))
    } else {None};
    let (code,dtype)=classification.map(|(c,t)|(Some(c),Some(t)))
        .unwrap_or_else(||(None,if doc.source_kind=="ID" {
            crate::catalog::guess_general_document_type(filename,&extraction.text)
        }else{None}));
    // Preserve engineer-selected type, all previous versions and original bytes.
    sqlx::query("UPDATE documents SET recognized_text=CASE WHEN $2 LIKE 'error:%' OR btrim($1)='' THEN recognized_text ELSE $1 END,recognition_status=$2,recognition_confidence=$3,
        document_type=COALESCE(document_type,$4),catalog_code=COALESCE(catalog_code,$5) WHERE id=$6")
        .bind(extraction.text).bind(&extraction.status).bind(extraction.confidence)
        .bind(dtype).bind(code).bind(doc.id).execute(db).await?;
    Ok(extraction.status)
}

pub async fn process_next(db:&PgPool,cfg:&Config,catalog:&Catalog)->Result<bool>{
    let Some(job)=claim_next(db).await? else {return Ok(false)};
    info!(job_id=job.id,document_id=job.document_id,package_id=job.package_id,"OCR job started");
    let outcome=process(db,cfg,catalog,&job).await;
    // Avoid generating a complete check run for each of 53 documents in one bulk job.
    // Run the package rule engine once when the last queued/running OCR item finishes.
    let remaining:(i64,)=sqlx::query_as(
        "SELECT count(*) FROM ocr_jobs WHERE package_id=$1 AND id<>$2 AND status IN ('pending','running')")
        .bind(job.package_id).bind(job.id).fetch_one(db).await?;
    if remaining.0==0 {
        write_event(db,job.id,"analysis",0,0,"Распознавание комплекта завершено. Пересчёт проверок...").await?;
        if let Err(err)=engine::analyze(db,catalog,job.package_id,"ocr_reprocess_web").await {
            warn!(job_id=job.id,"Rule analysis after OCR failed: {err:#}");
            write_event(db,job.id,"analysis_error",0,0,&format!("OCR сохранён, но пересчёт правил не выполнен: {err}")).await?;
        }
    }
    let (status,detail,error_message)=match outcome {
        Ok(result) if result.starts_with("error:")=>("failed",result.clone(),result),
        Ok(result) if result=="partial" || result=="manual"=>("partial",format!("Завершено с ограничениями: {result}"),String::new()),
        Ok(result)=>("done",format!("Успешно: {result}"),String::new()),
        Err(err)=>{
            error!(job_id=job.id,"OCR job failed: {err:#}");
            ("failed",format!("Ошибка задания: {err}"),err.to_string())
        }
    };
    sqlx::query("UPDATE ocr_jobs SET status=$2,stage=$2,message=$3,error_message=$4,
        updated_at=now(),finished_at=now() WHERE id=$1")
        .bind(job.id).bind(status).bind(detail.chars().take(320).collect::<String>())
        .bind(error_message.chars().take(1000).collect::<String>()).execute(db).await
        .with_context(||format!("Finishing OCR job {}",job.id))?;
    // Final event, status is preserved.
    sqlx::query("INSERT INTO ocr_job_events(job_id,stage,message) VALUES ($1,$2,$3)")
        .bind(job.id).bind(status).bind(&detail).execute(db).await?;
    if status=="done" || status=="partial" {
        if let Err(err)=crate::classification::maybe_auto_enqueue(db,cfg,job.document_id).await {
            warn!(document_id=job.document_id,"Could not auto-queue classification: {err:#}");
        }
    }
    info!(job_id=job.id,document_id=job.document_id,status,"OCR job completed");
    Ok(true)
}

pub async fn recover_interrupted(db:&PgPool)->Result<u64>{
    let recovered=sqlx::query("UPDATE ocr_jobs SET status='pending',stage='queued',
      message='Задание восстановлено после перезапуска',started_at=NULL,updated_at=now()
      WHERE status='running'").execute(db).await?.rows_affected();
    if recovered>0 {warn!(recovered,"Interrupted OCR jobs restored to queue");}
    Ok(recovered)
}

pub async fn print_status(db:&PgPool,cfg:&Config)->Result<()> {
    let counts:Vec<(String,i64)>=sqlx::query_as(
        "SELECT status,count(*) FROM ocr_jobs GROUP BY status ORDER BY status").fetch_all(db).await?;
    println!("OCR: mode={} local_models_ready={} model_dir={} max_pages={} page_workers={} VibeCode_enabled={} key_present={} timeout={}s",
        cfg.ocr_mode,crate::local_ocr::ensure_models(&cfg.local_ocr_model_dir).is_ok(),
        cfg.local_ocr_model_dir.display(),cfg.ocr_max_pages,cfg.ocr_page_workers,cfg.vibe_ocr_enabled,
        cfg.vibe_ocr_key.is_some(),cfg.vibe_ocr_timeout);
    println!("OCR jobs by status: {counts:?}");
    for job in jobs(db,None,20).await? {
        println!("#{} document={} status={} stage={} progress={}/{} updated={} message={}",
            job.id,job.document_id,job.status,job.stage,job.current_step,job.total_steps,
            job.updated_at.format("%Y-%m-%d %H:%M:%S UTC"),job.message);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn retry_only_problematic_documents(){
        assert!(needs_retry("error: pdftoppm unavailable"));
        assert!(needs_retry("partial"));
        assert!(needs_retry("manual"));
        assert!(!needs_retry("ocr_vibe"));
        assert!(!needs_retry("extracted"));
    }
}
