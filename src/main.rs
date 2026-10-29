mod ai;
mod access;
mod portal;
mod project_sources;
mod project_requirements;
mod semantic;
mod block1;
mod catalog;
mod classification;
mod config;
mod docs;
mod engine;
mod models;
mod local_ocr;
mod norms;
mod ocr_jobs;
mod repo;
mod reprocess;
mod reports;
mod scanner;
mod synology;
mod vibecode;
mod web;
mod ui_version;
mod contractor;

use anyhow::Result;
use rand::RngCore;
use sqlx::postgres::PgPoolOptions;
use std::{sync::{Arc,atomic::{AtomicBool,Ordering}}, time::Duration};
use tokio::{net::TcpListener,signal};
use tracing::{error,info};

#[tokio::main]
async fn main()->Result<()> {
    tracing_subscriber::fmt().with_env_filter(
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_|"id_control_axum=info,tower_http=info,info".into())
    ).init();
    let debug_login=std::env::args().any(|arg|arg=="--debug-login");
    if debug_login { tracing::warn!("DEBUG LOGIN ACTIVE: Basic admin/super123; do not expose this mode to the Internet"); }
    let cfg=config::Config::load()?;
    info!(data_root=%cfg.data_root.display(),temp_root=%cfg.temp_root.display(),models=%cfg.local_ocr_model_dir.display(),"Local Windows/project storage paths resolved");
    // Validate the local semantic model without touching PostgreSQL or Synology.
    if std::env::args().any(|arg|arg=="--check-semantic-model") {
        semantic::check_cli().await?;
        return Ok(());
    }
    // Stand-alone OCR test (no PostgreSQL or Synology required).
    if std::env::args().any(|arg|arg=="--check-local-models") {
        local_ocr::ensure_models(&cfg.local_ocr_model_dir)?;
        println!("Локальные модели найдены: {}",cfg.local_ocr_model_dir.display());
        return Ok(());
    }
    // Test one local JPG/PNG/TIFF/PDF without using PostgreSQL or Synology.
    let args:Vec<String>=std::env::args().collect();
    if let Some(pos)=args.iter().position(|a|a=="--check-ocr" || a=="--check-vibe-ocr" || a=="--check-local-ocr") {
        let file=args.get(pos+1).ok_or_else(||anyhow::anyhow!("Usage: --check-ocr path/to/image.tif"))?;
        let mut test_config=cfg.clone();
        if args[pos]=="--check-vibe-ocr" {test_config.ocr_mode="vibecode".into();}
        if args[pos]=="--check-local-ocr" {test_config.ocr_mode="local".into();}
        println!("OCR test: mode={} file={}",test_config.ocr_mode,file);
        let callback:docs::ProgressCallback=std::sync::Arc::new(|event|{
            println!("  [{}] {}/{}: {}",event.stage,event.current,event.total,event.message);
        });
        let result=docs::extract_with_progress(std::path::Path::new(file),test_config.ocr_max_pages,&test_config,Some(callback)).await;
        println!("Recognition status: {}",result.status);
        println!("EXTRACTED:\n{}",result.text);
        if result.status.starts_with("error:") {anyhow::bail!("OCR failed");}
        return Ok(());
    }
    // Test DSM connectivity without touching PostgreSQL or scanning/downloading files.
    if std::env::args().any(|arg|arg=="--check-synology" || arg=="--list-synology") {
        let nas=synology::SynologyClient::new(&cfg)?;
        let session=nas.connect().await?;
        let files=session.list_files().await?;
        session.logout().await;
        println!("Synology HTTPS connection OK. Files in configured folder: {}",files.len());
        if std::env::args().any(|arg|arg=="--list-synology") {
            for file in files.iter().take(50) {
                println!("{} ({} bytes)",file.path,file.size);
            }
            if files.len()>50 {println!("...and {} more",files.len()-50);}
        }
        return Ok(());
    }
    let catalog=catalog::Catalog::load()?;
    info!("Loaded {} work types and {} common errors",catalog.work_types.len(),catalog.errors.len());
    let db=PgPoolOptions::new().max_connections(8).connect(&cfg.database_url).await?;
    // Versioned, checksummed SQLx migrations; prevents accidental reapplication.
    sqlx::migrate!("./migrations").run(&db).await?;
    if std::env::args().any(|arg|arg=="--migrate-only") {
        println!("PostgreSQL migrations applied successfully");
        return Ok(());
    }
    if std::env::args().any(|arg|arg=="--reclassify-existing") {
        reprocess::classify_existing(&db,&catalog).await?;
        return Ok(());
    }
    if std::env::args().any(|arg|arg=="--ocr-status") {
        ocr_jobs::print_status(&db,&cfg).await?;
        return Ok(());
    }
    if std::env::args().any(|arg|arg=="--reprocess-errors" || arg=="--reprocess-all") {
        let all=std::env::args().any(|arg|arg=="--reprocess-all");
        reprocess::run(&db,&cfg,&catalog,all).await?;
        return Ok(());
    }

    // A restart may interrupt either the HTTP stream or the *post-publication*
    // OCR/indexing at 96%. Preserve NAS-backed deliveries and allow the existing
    // "Повторить импорт и проверку" operation to safely reconstruct the documents.
    sqlx::query("UPDATE contractor_submissions SET status='failed',processing_stage='failed',progress_updated_at=now(), \
        error_message=CASE WHEN nas_path IS NOT NULL AND jsonb_array_length(manifest_json)>0 \
        THEN 'Индексирование прервано перезапуском. Архив на Synology сохранён. Нажмите «Повторить импорт и проверку».' \
        ELSE 'Передача ZIP прервана перезапуском до публикации. Требуется новая ссылка подрядчику.' END \
        WHERE status='uploading'")
        .execute(&db).await?;
    // An interrupted check is retryable because its NAS original and manifest
    // remain available; never strand a delivery permanently in 'checking'.
    sqlx::query("UPDATE contractor_submissions SET status='pending_review',error_message='Проверка была прервана перезапуском сервиса. Запустите её повторно.' WHERE status='checking'")
        .execute(&db).await?;
    ocr_jobs::recover_interrupted(&db).await?;
    classification::recover_interrupted(&db).await?;
    // Resume jobs interrupted by an application restart.
    sqlx::query("UPDATE ai_jobs SET status='pending', started_at=NULL WHERE status='running'")
        .execute(&db).await?;
    let mut bytes=[0u8;32];rand::thread_rng().fill_bytes(&mut bytes);
    let csrf=hex::encode(bytes);
    let nas=synology::SynologyClient::new(&cfg)?;
    let st=Arc::new(web::AppState{cfg:cfg.clone(),db,catalog,nas,
        synology_connected:AtomicBool::new(false),scan_lock:tokio::sync::Mutex::new(()),csrf,
        scan_status:std::sync::RwLock::new(web::ScanStatus::default()),debug_login});
    project_sources::recover_interrupted(&st).await?;
    semantic::recover(&st.db).await?;
    // A manual generation interrupted by a previous process crash is retryable.
    sqlx::query("UPDATE project_requirement_generation_jobs SET status='error',error_message='Анализ прерван перезапуском. Нажмите «Сформировать требования» повторно.',finished_at=now() WHERE status='running'")
        .execute(&st.db).await?;
    // Backfill proposal candidates from previously OCR-completed project sources
    // without re-uploading or re-running OCR on originals.
    let backfill=st.clone();
    tokio::spawn(async move {
        match sqlx::query_scalar::<_,i64>("SELECT id FROM projects").fetch_all(&backfill.db).await {
            Ok(ids) => for id in ids {
                if let Err(e)=project_requirements::refresh_project(&backfill.db,&backfill.catalog,id).await {
                    tracing::error!(project_id=id,"Project requirements backfill failed: {e:#}");
                }
            },
            Err(e)=>tracing::error!("Project requirements startup query failed: {e:#}"),
        }
    });
    let background=st.clone();
    tokio::spawn(async move{
        let mut interval=tokio::time::interval(Duration::from_secs(background.cfg.poll_interval));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop{
            interval.tick().await;
            let _guard=background.scan_lock.lock().await;
            if let Ok(mut scan)=background.scan_status.write() {
                scan.running=true;
                scan.message="Идёт сканирование Synology".into();
            }
            info!("Synology scan started");
            match scanner::scan_once(&background.db,&background.cfg,&background.catalog,&background.nas).await {
                Ok(result)=>{
                    background.synology_connected.store(true,Ordering::Relaxed);
                    if let Ok(mut scan)=background.scan_status.write() {
                        scan.running=false;
                        scan.last_at=Some(chrono::Utc::now());
                        scan.message=format!("Synology: {} новых/обновлённых документов",result.new_documents);
                        scan.detail=result.summary();
                    }
                },
                Err(e)=>{
                    background.synology_connected.store(false,Ordering::Relaxed);
                    error!("Synology scan failed: {e:#}");
                    if let Ok(mut scan)=background.scan_status.write() {
                        scan.running=false;
                        scan.last_at=Some(chrono::Utc::now());
                        scan.message=format!("Ошибка сканирования: {}",e.to_string().chars().take(230).collect::<String>());
                        scan.detail=String::new();
                    }
                },
            }
        }
    });
    let ocr_background=st.clone();
    tokio::spawn(async move {
        let mut interval=tokio::time::interval(Duration::from_secs(2));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(err)=ocr_jobs::process_next(&ocr_background.db,&ocr_background.cfg,&ocr_background.catalog).await {
                error!("OCR background worker error: {err:#}");
            }
        }
    });
    let classify_background=st.clone();
    tokio::spawn(async move {
        let mut interval=tokio::time::interval(Duration::from_secs(3));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(e)=classification::process_next(&classify_background.db,&classify_background.cfg).await {
                error!("Classification worker error: {e:#}");
            }
        }
    });
    let ai_background=st.clone();
    tokio::spawn(async move {
        let mut interval=tokio::time::interval(Duration::from_secs(10));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(e)=ai::process_next_job(&ai_background).await {
                error!("AI job runner error: {e:#}");
            }
        }
    });
    let addr=cfg.listen.parse::<std::net::SocketAddr>()?;
    // `--debug-login` is an explicit operator choice; honor configured LISTEN on any interface.
    let listener=TcpListener::bind(addr).await?;
    info!("Listening at http://{addr}; put HTTPS reverse proxy in front of Basic authentication");
    axum::serve(listener,web::router(st)).with_graceful_shutdown(async{let _=signal::ctrl_c().await;}).await?;
    Ok(())
}
