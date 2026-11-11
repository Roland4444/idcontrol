//! DB-backed OCR queue, single claim per document, persisted progress.
//! Only non-sensitive metadata is logged: no key, document text, or image bytes.
use crate::{catalog::Catalog, config::Config, docs, engine, repo};
use anyhow::{anyhow, Context, Result};

use chrono::{DateTime,Utc};
use serde::Serialize;
use sqlx::{FromRow,PgPool};
use std::{path::{Path, PathBuf},  sync::Arc};
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

pub async fn jobs_filtered(db:&PgPool,package_id:Option<i64>,status:Option<&str>,limit:i64)->Result<Vec<OcrJob>>{
    let status=status.filter(|s|matches!(*s,"pending"|"running"|"done"|"partial"|"failed"|"cancelled"));
    let sql=format!("SELECT {JOB_COLUMNS} FROM ocr_jobs WHERE ($1::bigint IS NULL OR package_id=$1) AND ($2::text IS NULL OR status=$2) ORDER BY CASE status WHEN 'running' THEN 0 WHEN 'pending' THEN 1 ELSE 2 END, id DESC LIMIT $3");
    Ok(sqlx::query_as::<_,OcrJob>(&sql).bind(package_id).bind(status).bind(limit.clamp(1,200)).fetch_all(db).await?)
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


use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};
use rapidocr_core::{
    config::{InferenceOptions, PipelineConfig},
    model::{model_set_by_name, ModelCache, ModelDownloadMode},
    RapidOcr,
};
use regex::Regex;
use tempfile::TempDir;

/// Рендерит первую страницу PDF в PNG и возвращает путь к временному файлу.
// fn render_first_pdf_page_to_png(pdf_path: &Path, work_dir: &Path) -> Result<std::path::PathBuf> {
//     let bytes = std::fs::read(pdf_path)
//         .with_context(|| format!("не удалось прочитать PDF: {}", pdf_path.display()))?;

//     let pdf = hayro::hayro_syntax::Pdf::new(bytes)
//         .map_err(|e| anyhow::anyhow!("hayro не смог разобрать PDF: {e:?}"))?;

//     let page = pdf.pages().first()
//         .context("PDF не содержит страниц")?;

//     let cache = hayro::RenderCache::new();
//     let interpreter = hayro::hayro_interpret::InterpreterSettings::default();

//     // 2.0 ≈ 144 DPI — достаточно для OCR, но без огромных PNG.
//     let pixmap = hayro::render(
//         page,
//         &cache,
//         &interpreter,
//         &hayro::RenderSettings {
//             x_scale: 2.0,
//             y_scale: 2.0,
//             bg_color: hayro::vello_cpu::color::palette::css::WHITE,
//             ..Default::default()
//         },
//     );

//     let width = pixmap.width() as u32;
//     let height = pixmap.height() as u32;

//     let png_path = work_dir.join("page-0.png");
//     let file = std::fs::File::create(&png_path)
//         .with_context(|| format!("не удалось создать PNG: {}", png_path.display()))?;

//     PngEncoder::new(file)
//         .write_image(
//             pixmap.data_as_u8_slice(),
//             width,
//             height,
//             ExtendedColorType::Rgba8,
//         )
//         .context("не удалось закодировать PNG")?;

//     Ok(png_path)
// }

use hayro::{PixmapSettings, RenderSettings};

fn render_first_pdf_page_to_png(pdf_path: &Path, work_dir: &Path) -> Result<std::path::PathBuf> {
    let bytes = std::fs::read(pdf_path)
        .with_context(|| format!("не удалось прочитать PDF: {}", pdf_path.display()))?;

    let pdf = hayro::hayro_syntax::Pdf::new(bytes)
        .map_err(|e| anyhow::anyhow!("hayro не смог разобрать PDF: {e:?}"))?;

    let page = pdf.pages().first()
        .context("PDF не содержит страниц")?;

    let cache = hayro::RenderCache::new();
    let interpreter = hayro::hayro_interpret::InterpreterSettings::default();

    let render_settings = RenderSettings {
        force_image_interpolation: false,
        ..Default::default()
    };

    // В hayro 0.8 масштаб и фон относятся уже к PixmapSettings.
    let pixmap_settings = PixmapSettings {
        x_scale: 2.0, // ≈ 144 DPI
        y_scale: 2.0,
        bg_color: hayro::vello_cpu::color::palette::css::WHITE,
    };

    let pixmap = hayro::render(
        page,
        &cache,
        &interpreter,
        &render_settings,
        &pixmap_settings,
    );

    let width = pixmap.width() as u32;
    let height = pixmap.height() as u32;

    let png_path = work_dir.join("page-0.png");
    let file = std::fs::File::create(&png_path)
        .with_context(|| format!("не удалось создать PNG: {}", png_path.display()))?;

    PngEncoder::new(file)
        .write_image(
            pixmap.data_as_u8_slice(),
            width,
            height,
            ExtendedColorType::Rgba8,
        )
        .context("не удалось закодировать PNG")?;

    Ok(png_path)
}





/// Создаёт OCR-движок RapidOCR. Модели должны лежать в `models/`
/// или будут скачаны при первом запуске.
fn build_ocr() -> Result<RapidOcr> {
    let model_set = model_set_by_name("ppocrv6-small")
        .context("неизвестный набор моделей RapidOCR")?;

    let cache = ModelCache::new("models");

    cache.ensure_model_set_for_pipeline(
        model_set,
        PipelineConfig::without_cls(),
        ModelDownloadMode::Missing,
    )?;

    let cfg = cache
        .config_for(model_set)
        .with_pipeline(PipelineConfig::without_cls())
        .with_inference_options(InferenceOptions {
            intra_threads: 1,
            inter_threads: 1,
            parallel_execution: false,
            ..Default::default()
        });

    RapidOcr::from_config(cfg).context("не удалось создать RapidOCR")
}

/// Извлекает ИНН из распознанного текста.
/// Поддерживает 10- и 12-значный ИНН, метки «ИНН» / «INN».
fn extract_inn_from_text(text: &str) -> Option<String> {
    // OCR путает: И→I/H/N/É/È, Н→H/N, М→M.
    // Ищем 2–3 «похожих» символа, потом разделители и 10/12 цифр.
    let re = Regex::new(r"(?i)[ИIНHNМMÉÈ]{2,3}[\s:№#\-]*(\d[\d\s]{8,15}\d)").unwrap();
    re.captures(text).and_then(|c| {
        let digits: String = c[1].chars().filter(|ch| ch.is_ascii_digit()).collect();
        match digits.len() {
            10 | 12 => Some(digits),
            _ => None,
        }
    })
}
/// Основная функция: принимает путь к PDF и возвращает ИНН, если он найден.
// pub fn extract_inn_from_pdf(pdf_path: &Path) -> Result<Option<String>> {
//     let work_dir = TempDir::new().context("не удалось создать временную директорию")?;

//     let png_path = render_first_pdf_page_to_png(pdf_path, work_dir.path())?;

//     let mut ocr = build_ocr()?;
//     let result = ocr
//         .run_path(png_path.to_str().context("путь к PNG не является UTF-8")?)
//         .context("OCR не смог обработать изображение")?;

//     // Склеиваем строки с достаточной уверенностью.
//     let text: String = result
//         .lines
//         .into_iter()
//         .filter(|line| line.score > 0.3)
//         .map(|line| line.text)
//         .collect::<Vec<_>>()
//         .join("\n");

//     Ok(extract_inn_from_text(&text))
// }

pub fn extract_inn_from_pdf(pdf_path: &Path) -> Result<Option<String>> {
    let work_dir = TempDir::new().context("не удалось создать временную директорию")?;
    let png_path = render_first_pdf_page_to_png(pdf_path, work_dir.path())?;
    let png_bytes = std::fs::read(&png_path)
        .with_context(|| format!("не удалось прочитать PNG: {}", png_path.display()))?;

    // Продакшен-путь: local_ocr с eslav-моделями из models/ocr/
    let model_dir = PathBuf::from("models/ocr");
    let pages = crate::local_ocr::recognize_pages(
        vec![png_bytes],
        model_dir,
        1,    // один поток
        None,
    )?;

    let text = pages.into_iter().map(|p| p.text).collect::<Vec<_>>().join("\n");
    std::fs::write("debug_ocr.txt", &text).ok();
    println!("--- OCR ---\n{}\n--- /OCR ---", text);

    Ok(extract_inn_from_text(&text))
}




// pub fn extract_inn_from_pdf(pdf_path: &Path) -> Result<Option<String>> {
//     let work_dir = TempDir::new().context("не удалось создать временную директорию")?;

//     let png_path = render_first_pdf_page_to_png(pdf_path, work_dir.path())?;

//     // Сохраняем PNG рядом с проектом для визуальной проверки
//     std::fs::copy(&png_path, "debug_page0.png")
//         .context("не удалось сохранить debug_page0.png")?;

//     let mut ocr = build_ocr()?;
//     let result = ocr
//         .run_path(png_path.to_str().context("путь к PNG не является UTF-8")?)
//         .context("OCR не смог обработать изображение")?;

//     // Печатаем все строки с их confidence, без фильтра
//     for line in &result.lines {
//         println!("[{:.3}] {}", line.score, line.text);
//     }

//     let text: String = result
//         .lines
//         .into_iter()
//         .filter(|line| line.score > 0.3)
//         .map(|line| line.text)
//         .collect::<Vec<_>>()
//         .join("\n");

//     std::fs::write("debug_ocr.txt", &text)
//         .context("не удалось сохранить debug_ocr.txt")?;

//     Ok(extract_inn_from_text(&text))
// }

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




    #[test]
    fn extracts_10_digit_inn() {
        assert_eq!(
            extract_inn_from_text("ИНН 7707083893"),
            Some("7707083893".to_string())
        );
    }

    #[test]
    fn extracts_12_digit_inn() {
        assert_eq!(
            extract_inn_from_text("ИНН: 770708389312"),
            Some("770708389312".to_string())
        );
    }

    #[test]
    fn extracts_inn_with_latin_label() {
        assert_eq!(
            extract_inn_from_text("INN 1234567890"),
            Some("1234567890".to_string())
        );
    }

    #[test]
    fn returns_none_when_no_inn() {
        assert_eq!(extract_inn_from_text("Договор № 42"), None);
    }

    /// Интеграционный тест: требует скачанных моделей RapidOCR
    /// и файла `test_data/sample_invoice.pdf` с известным ИНН.
    /// Запуск: `cargo test -- --ignored`
    #[test]
    #[ignore = "требует models/ и test_data/sample_invoice.pdf"]
    fn extracts_inn_from_real_pdf() -> anyhow::Result<()> {
        let path = std::path::Path::new("test_data/file-114098.pdf");
        let inn = extract_inn_from_pdf(path)?;
        assert_eq!(inn.as_deref(), Some("7707083893"));
        Ok(())
    }


    #[test]
    #[ignore = "требует models/ и test_data/sample_invoice.pdf"]
    fn extracts_inn_from_direct_pdf() -> anyhow::Result<()> {
        let path = std::path::Path::new("test_data/dirext.pdf");
        let inn = extract_inn_from_pdf(path)?;
        assert_eq!(inn.as_deref(), Some("7707083893"));
        Ok(())
    }


    #[test]
    #[ignore = "требует models/ и test_data/sample_invoice.pdf"]
    fn extracts_inn_from_1t_pdf() -> anyhow::Result<()> {
        let path = std::path::Path::new("test_data/1.pdf");
        let inn = extract_inn_from_pdf(path)?;
        assert_eq!(inn.as_deref(), Some("7707083893"));
        Ok(())
    }
}
    
    


