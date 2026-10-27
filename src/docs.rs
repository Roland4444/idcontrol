//! DOCX/XLSX via local text extraction; PDF/images via offline neural ONNX OCR
//! or explicit opt-in VibeCode. PDF renders with Hayro, no external tools.
use crate::{config::Config,vibecode::VibeOcr};
use anyhow::{anyhow, bail, Result};
use calamine::{open_workbook_auto, Reader};
use hayro::{render, RenderCache, RenderSettings, PixmapSettings};
use hayro::{hayro_interpret::InterpreterSettings,hayro_syntax::Pdf};
use image::{imageops, DynamicImage, ImageFormat, Rgba, RgbaImage};
use regex::Regex;
use std::{fs::{self,File}, io::Read, path::{Path, PathBuf}, sync::{Arc, OnceLock}, time::{Duration, Instant}};
use tokio::process::Command;
use tokio::sync::Semaphore;
use zip::ZipArchive;

pub const SUPPORTED: &[&str] = &["doc","docx","xls","xlsx","pdf","png","jpg","jpeg","tif","tiff","bmp","gif","webp","dwg","dxf"];
pub fn supported(p:&Path) -> bool { p.extension().and_then(|x|x.to_str()).map(|s|SUPPORTED.contains(&s.to_lowercase().as_str())).unwrap_or(false) }

#[derive(Clone, Default)]
pub struct Extracted { pub text: String, pub status:String, pub confidence:Option<f32> }

/// No document contents or credentials are sent to progress listeners.
#[derive(Debug, Clone)]
pub struct OcrProgress {
    pub stage: &'static str,
    pub current: i32,
    pub total: i32,
    pub message: String,
}
pub type ProgressCallback = Arc<dyn Fn(OcrProgress) + Send + Sync>;
fn progress(cb:&Option<ProgressCallback>, stage:&'static str, current:i32, total:i32, message:impl Into<String>) {
    if let Some(cb)=cb {cb(OcrProgress{stage,current,total,message:message.into()});}
}


fn unzip_member(path:&Path,member:&str)->Result<String>{
    let mut archive=ZipArchive::new(File::open(path)?)?;
    let mut body=String::new();
    archive.by_name(member)?.read_to_string(&mut body)?;
    Ok(body)
}
/// DOCX splits words across XML runs; joining them with spaces corrupts text.
fn docx_xml_to_text(xml:&str)->Result<String>{
    let tokens=Regex::new(r"(?s)<w:t(?:\s[^>]*)?>(.*?)</w:t>|</w:p\s*>|<w:tab(?:\s[^>]*)?\s*/>|<w:br(?:\s[^>]*)?\s*/>")?;
    let mut text=String::new();
    for capture in tokens.captures_iter(xml) {
        if let Some(fragment)=capture.get(1) { text.push_str(&html_escape::decode_html_entities(fragment.as_str())); }
        else if let Some(tag)=capture.get(0) { if tag.as_str().starts_with("<w:tab") {text.push('\t');}else{text.push('\n');} }
    }
    Ok(text)
}
fn docx_text(path:&Path)->Result<String>{docx_xml_to_text(&unzip_member(path,"word/document.xml")?)}
/// Calamine automatically selects both OLE2 XLS and modern XLSX formats.
fn spreadsheet_text(path:&Path)->Result<String>{
    let mut wb=open_workbook_auto(path)?;
    let mut out=String::new();
    for name in wb.sheet_names().to_vec(){
        out.push_str(&format!("Лист: {name}\n"));
        let range=wb.worksheet_range(&name)?;
        for (n,row) in range.rows().enumerate(){
            if n>=10000 {break;}
            let vals:Vec<String>=row.iter().map(ToString::to_string).filter(|s|!s.trim().is_empty()).collect();
            if !vals.is_empty(){out.push_str(&vals.join(" | "));out.push('\n');}
        }
    }
    Ok(out)
}

/// LibreOffice is invoked by the application (no manual conversion by an operator).
/// An isolated profile and source copy prevent changes to the original DOC.
/// The executable can be configured on Windows or Linux with LIBREOFFICE_BIN.
fn libreoffice_executable() -> PathBuf {
    if let Ok(path)=std::env::var("LIBREOFFICE_BIN") {
        if !path.trim().is_empty() { return PathBuf::from(path); }
    }
    if cfg!(windows) {
        for key in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(root)=std::env::var_os(key) {
                let path=PathBuf::from(root).join("LibreOffice").join("program").join("soffice.exe");
                if path.is_file() { return path; }
            }
        }
    }
    PathBuf::from("soffice")
}

/// Converts a staged legacy DOC to either DOCX (text extraction) or PDF
/// (OCR fallback for image-only content). Uses argument-safe Command, with
/// no shell and a separate LO user profile for simultaneous uploads.
async fn convert_legacy_doc(staged:&Path, outdir:&Path, format:&str) -> Result<PathBuf> {
    let (format_arg,extension)=match format {
        "docx" => ("docx:Office Open XML Text", "docx"),
        "pdf" => ("pdf:writer_pdf_Export", "pdf"),
        _ => bail!("Unsupported DOC conversion format"),
    };
    let profile=tempfile::tempdir_in(outdir)?;
    let profile_url=url::Url::from_directory_path(profile.path())
        .map_err(|_|anyhow!("Cannot form LibreOffice user profile URI"))?;
    let executable=libreoffice_executable();
    let mut cmd=Command::new(&executable);
    cmd.arg(format!("-env:UserInstallation={profile_url}"))
        .arg("--headless")
        .arg("--nologo")
        .arg("--nodefault")
        .arg("--norestore")
        .arg("--convert-to").arg(format_arg)
        .arg("--outdir").arg(outdir)
        .arg(staged)
        .kill_on_drop(true);
    let result=tokio::time::timeout(Duration::from_secs(120),cmd.output()).await
        .map_err(|_|anyhow!("LibreOffice DOC conversion exceeded 120 seconds"))?
        .map_err(|err|anyhow!("Could not start LibreOffice ({}): {err}. Install libreoffice-writer or set LIBREOFFICE_BIN",executable.display()))?;
    let target=outdir.join(format!("input.{extension}"));
    if !result.status.success() || !target.is_file() {
        let diagnostic=String::from_utf8_lossy(&result.stderr);
        bail!("LibreOffice could not convert old Word DOC to {format}: {}",
            diagnostic.chars().take(300).collect::<String>());
    }
    Ok(target)
}

// Do not start an unbounded number of LibreOffice processes when many
// contractor submissions are scanned at the same time.
static DOC_CONVERSION_SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
fn doc_conversion_slots() -> Arc<Semaphore> {
    Arc::clone(DOC_CONVERSION_SLOTS.get_or_init(|| Arc::new(Semaphore::new(2))))
}

async fn legacy_doc_extract(path:&Path,max_pages:usize,cfg:&Config,cb:&Option<ProgressCallback>)->Result<Extracted>{
    let conversion_permit=doc_conversion_slots().acquire_owned().await?;
    progress(cb,"convert",0,2,"Автоматическая конвертация старого Word DOC для проверки");
    let temporary=tempfile::tempdir_in(&cfg.temp_root)?;
    let staged=temporary.path().join("input.doc");
    // Keep the unmodified original in the immutable archive; only convert a copy.
    tokio::fs::copy(path,&staged).await?;
    let converted=convert_legacy_doc(&staged,temporary.path(),"docx").await?;
    let text=tokio::task::spawn_blocking(move ||docx_text(&converted)).await??;
    if !text.trim().is_empty() {
        progress(cb,"extract",2,2,"DOC конвертирован: текст извлечён и передан на нормативную проверку");
        return Ok(Extracted{text,status:"extracted".into(),confidence:None});
    }
    // Some OLE2 documents consist solely of scanned pictures. Render the
    // source to PDF and pass through the existing OCR pipeline automatically.
    progress(cb,"convert",1,2,"В DOC нет текстового слоя: подготовка страниц к OCR");
    let pdf=convert_legacy_doc(&staged,temporary.path(),"pdf").await?;
    drop(conversion_permit);
    let parsed=Box::pin(run_extract(&pdf,max_pages,cfg,cb)).await?;
    progress(cb,"extract",2,2,"Обработка DOC через OCR завершена");
    Ok(parsed)
}

/// Convert a rendered PDF page (PNG) to a real JPEG.
/// Original TIFF/BMP/GIF/JPG/PNG files bypass this function entirely.
fn image_as_jpeg(bytes:&[u8],source_format:ImageFormat)->Result<Vec<u8>>{
    let mut im=image::load_from_memory_with_format(bytes,source_format)?;
    // Bound memory and HTTP image payload. Downsampling can reduce very fine print,
    // so this value is deliberately larger than a normal web thumbnail.
    if im.width()>2600 || im.height()>2600 {
        im=im.resize(2600,2600,imageops::FilterType::Lanczos3);
    }
    let mut white=RgbaImage::from_pixel(im.width(),im.height(),Rgba([255,255,255,255]));
    imageops::overlay(&mut white,&im.to_rgba8(),0,0);
    let rgb=DynamicImage::ImageRgba8(white).to_rgb8();
    let mut result=Vec::new();
    let mut encoder=image::codecs::jpeg::JpegEncoder::new_with_quality(&mut result,88);
    encoder.encode_image(&DynamicImage::ImageRgb8(rgb))?;
    if result.len()>20*1024*1024 {bail!("Rendered image exceeds VibeCode 20 MiB size limit");}
    Ok(result)
}

/// CPU-only PDF renderer implemented in Rust (Hayro), no external executables.
/// Work is executed with spawn_blocking by the caller to avoid blocking Tokio.
fn pdf_pages_as_jpegs(path:&Path,max_pages:usize,cb:&Option<ProgressCallback>)->Result<(Vec<Vec<u8>>,usize)> {
    let bytes=fs::read(path)?;
    let pdf=Pdf::new(bytes).map_err(|e|anyhow!("Cannot parse PDF: {e:?}"))?;
    let pages=pdf.pages();
    let total=pages.len();
    if total==0 {bail!("PDF has no pages");}
    let settings = RenderSettings::default();
    let pixmap_settings = PixmapSettings {
        x_scale: 2.0,
        y_scale: 2.0,
        ..Default::default()
    };
    let interpreter=InterpreterSettings::default();
    let cache=RenderCache::new();
    let planned=max_pages.min(total);
    progress(cb,"render",0,planned as i32,format!("PDF: {total} страниц, подготовка {planned} к OCR"));
    let mut out=Vec::with_capacity(planned);
    for (index,page) in pages.iter().take(planned).enumerate() {
        progress(cb,"render",index as i32,planned as i32,format!("Рендеринг PDF, страница {} из {planned}",index+1));
        let pixmap = render(page, &cache, &interpreter, &settings, &pixmap_settings);
        let png=pixmap.into_png().map_err(|e|anyhow!("PDF PNG conversion failed: {e:?}"))?;
        out.push(image_as_jpeg(&png,ImageFormat::Png)?);
        progress(cb,"render",(index+1) as i32,planned as i32,format!("Страница {} из {planned} подготовлена",index+1));
    }
    Ok((out,total))
}

/// Count IFDs in a classical TIFF without decompressing all pages.
/// Unsupported BigTIFF/invalid headers return None so the result is marked partial.
fn tiff_page_count(bytes:&[u8])->Option<usize>{
    if bytes.len()<8 {return None;}
    let little=if &bytes[0..2]==b"II" {true}
        else if &bytes[0..2]==b"MM" {false} else {return None};
    let u16_at=|offset:usize|->Option<u16>{
        let b=bytes.get(offset..offset.checked_add(2)?)?;
        Some(if little {u16::from_le_bytes([b[0],b[1]])}else{u16::from_be_bytes([b[0],b[1]])})
    };
    let u32_at=|offset:usize|->Option<u32>{
        let b=bytes.get(offset..offset.checked_add(4)?)?;
        Some(if little {u32::from_le_bytes([b[0],b[1],b[2],b[3]])}
            else {u32::from_be_bytes([b[0],b[1],b[2],b[3]])})
    };
    if u16_at(2)?!=42{return None;}
    let mut offset=u32_at(4)? as usize;
    let mut count=0;
    let mut visited=std::collections::HashSet::new();
    while offset!=0 {
        if count>=1024 || !visited.insert(offset){return None;}
        let n=u16_at(offset)? as usize;
        let next=offset.checked_add(2)?.checked_add(n.checked_mul(12)?)?;
        offset=u32_at(next)? as usize;
        count+=1;
    }
    Some(count)
}

/// MIME used for selecting images. For TIFF/BMP/GIF the VibeCode client
/// deliberately uses data:image/jpeg in the URI (compatible with user test)
/// while keeping the original image bytes unchanged.
fn image_extension_mime(suffix:&str)->Result<&'static str>{
    match suffix {
        "jpg"|"jpeg"=>Ok("image/jpeg"),
        "png"=>Ok("image/png"),
        "tif"|"tiff"=>Ok("image/tiff"),
        "bmp"=>Ok("image/bmp"),
        "gif"=>Ok("image/gif"),
        "webp"=>Ok("image/webp"),
        _=>bail!("Unsupported image extension: {suffix}")
    }
}

async fn recognize_with_heartbeat(
    ocr:&VibeOcr, raw:&[u8],mime:&str,cb:&Option<ProgressCallback>,
    current:i32,total:i32
)->Result<crate::vibecode::TextResult>{
    let started=Instant::now();
    let future=ocr.recognize_image(raw,mime);
    tokio::pin!(future);
    let mut timer=tokio::time::interval(Duration::from_secs(15));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    timer.tick().await;
    loop {
        tokio::select! {
            result=&mut future => return result,
            _=timer.tick() => {
                progress(cb,"waiting_api",current,total,format!("Ожидание ответа VibeCode: {} сек. (таймаут на попытку: {} сек.)",started.elapsed().as_secs(),ocr.timeout_seconds));
            }
        }
    }
}

/// A process-wide permit pool bounds the number of ONNX sessions used by
/// overlapping scanner / web / CLI jobs in one process.
static LOCAL_OCR_SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();
fn local_ocr_slots(configured: usize) -> Arc<Semaphore> {
    Arc::clone(LOCAL_OCR_SLOTS.get_or_init(|| Arc::new(Semaphore::new(configured))))
}

/// Offline ONNX OCR. PDF pages are rendered one by one and pushed into a
/// bounded queue: the app does NOT buffer 500 rendered pages in memory.
async fn run_local_extract(path:&Path,max_pages:usize,cfg:&Config,cb:&Option<ProgressCallback>)->Result<Extracted>{
    let extension=path.extension().and_then(|s|s.to_str()).unwrap_or("").to_ascii_lowercase();
    let multi_tiff=if extension=="tif" || extension=="tiff" {
        let src=std::fs::read(path)?;
        tiff_page_count(&src)!=Some(1)
    }else{false};
    let workers=cfg.ocr_page_workers;
    let requested=if extension=="pdf" {workers} else {1};
    progress(cb,"local_wait",0,0,format!("Ожидание CPU-слотов локального OCR (запрошено {requested}, всего {workers})"));
    // Hold permits through OCR completion to prevent accidental oversubscription
    // when scanner and background OCR run simultaneously.
    let _permit=local_ocr_slots(workers).acquire_many_owned(requested as u32).await?;
    let started=Instant::now();
    let directory=cfg.local_ocr_model_dir.clone();
    let callback=cb.clone();
    let (pages,partial_pdf)=if extension=="pdf" {
        let filename=path.to_owned();
        tokio::task::spawn_blocking(move || -> Result<(Vec<crate::local_ocr::PageText>,bool)> {
            let bytes=fs::read(&filename)?;
            let pdf=Pdf::new(bytes).map_err(|e|anyhow!("Cannot parse PDF: {e:?}"))?;
            let all=pdf.pages().len();
            if all==0 {bail!("PDF has no pages");}
            let planned=all.min(max_pages);
            let truncated=planned<all;
            progress(&callback,"render",0,planned as i32,
                format!("PDF: {all} страниц, распознаём {planned}, параллельных OCR-потоков {workers}"));
            if truncated {progress(&callback,"partial",0,planned as i32,
                format!("Применён OCR_MAX_PAGES={max_pages}, документ будет помечен как частичный"));}
            let settings=RenderSettings::default();
            let pixmap_settings=PixmapSettings{x_scale:2.0,y_scale:2.0,..Default::default()};
            let interpreter=InterpreterSettings::default();
            let cache=RenderCache::new();
            let recognized=crate::local_ocr::recognize_stream(
                planned,&directory,workers,callback.clone(),|emit| {
                    for (index,page) in pdf.pages().iter().take(planned).enumerate() {
                        progress(&callback,"render",0,planned as i32,
                            format!("PDF: рендеринг страницы {} из {planned}",index+1));
                        let pixmap=render(page,&cache,&interpreter,&settings,&pixmap_settings);
                        let png=pixmap.into_png().map_err(|e|anyhow!("PDF PNG conversion failed on page {}: {e:?}",index+1))?;
                        let jpeg=image_as_jpeg(&png,ImageFormat::Png)?;
                        emit(index,jpeg)?; // blocks when queue is full, controlling RAM
                        progress(&callback,"render_queued",0,planned as i32,
                            format!("PDF: страница {} поставлена в OCR-очередь",index+1));
                    }
                    Ok(())
                }
            )?;
            Ok((recognized,truncated))
        }).await??
    }else{
        progress(cb,"preparing",0,1,"Чтение исходного изображения для локального OCR");
        let bytes=tokio::fs::read(path).await?;
        let recognized=tokio::task::spawn_blocking(move ||
            crate::local_ocr::recognize_pages(vec![bytes],directory,1,callback)
        ).await??;
        (recognized,false)
    };
    let mut text_parts=Vec::new();
    let mut scores=Vec::new();
    let mut empty=false;
    for (i,item) in pages.iter().enumerate() {
        if item.text.trim().is_empty() {empty=true;}
        else {
            text_parts.push(format!("=== Страница {} ===\n{}",i+1,item.text));
            if let Some(confidence)=item.confidence {scores.push(confidence);}
        }
    }
    progress(cb,"local_done",pages.len() as i32,pages.len() as i32,
        format!("Локальный OCR: {:.1} с, {} страниц, {:.1} страниц/мин",
            started.elapsed().as_secs_f64(),pages.len(),
            pages.len() as f64 * 60.0 / started.elapsed().as_secs_f64().max(0.01)));
    let text=text_parts.join("\n\n");
    let confidence=if scores.is_empty(){None}else{Some(scores.iter().sum::<f32>()/scores.len() as f32)};
    let status=if text.is_empty(){"manual"}else if partial_pdf || multi_tiff || empty {"partial"}else{"ocr_local"};
    Ok(Extracted{text,status:status.into(),confidence})
}

async fn run_extract(path:&Path,max_pages:usize,cfg:&Config,cb:&Option<ProgressCallback>)->Result<Extracted>{
    let suffix=path.extension().and_then(|s|s.to_str()).unwrap_or("").to_lowercase();
    // CAD binaries are preserved and versioned, but text extraction requires
    // specialist CAD tools. Never submit a DWG/DXF to image OCR as if it were a bitmap.
    if suffix=="dwg" || suffix=="dxf" {
        progress(cb,"extract",1,1,"Чертёж CAD сохранён; требуется просмотр инженером");
        return Ok(Extracted{text:String::new(),status:"manual".into(),confidence:None});
    }
    // A legacy Word file is converted automatically. The original remains
    // untouched for SHA-256/versioning and audits on Synology.
    if suffix=="doc" {
        return legacy_doc_extract(path,max_pages,cfg,cb).await;
    }
    if suffix=="docx" || suffix=="xlsx" || suffix=="xls" {
        progress(cb,"extract",0,1,format!("Автоматическое извлечение текста из {}",suffix.to_uppercase()));
        let file=path.to_owned();
        let text=tokio::task::spawn_blocking(move||{
            if suffix=="docx" {docx_text(&file)} else {spreadsheet_text(&file)}
        }).await??;
        progress(cb,"extract",1,1,"Извлечение текста из офисного документа завершено");
        return Ok(Extracted{text:text.clone(),status:if text.trim().is_empty(){"manual"}else{"extracted"}.into(),confidence:None});
    }
    progress(cb,"preparing",0,0,format!("Режим OCR: {}",cfg.ocr_mode));
    if cfg.ocr_mode=="local" {return run_local_extract(path,max_pages,cfg,cb).await;}
    if cfg.ocr_mode=="local_then_vibecode" {
        match run_local_extract(path,max_pages,cfg,cb).await {
            Ok(result) if result.status!="manual" => return Ok(result),
            Ok(_) => progress(cb,"fallback",0,0,"Локальный OCR не нашёл текста, переход к VibeCode (внешний API)"),
            Err(err) => progress(cb,"fallback",0,0,format!("Локальный OCR не сработал: {err:#}. Переход к VibeCode")),
        }
    }
    if !cfg.vibe_ocr_enabled {bail!("VibeCode OCR disabled (для внешнего режима установи VIBE_OCR_ENABLED=true и VIBE_OCR_KEY)");}
    let ocr=VibeOcr::new(&cfg.vibe_ocr_url,cfg.vibe_ocr_key.as_deref().unwrap_or(""),
        &cfg.vibe_ocr_model,cfg.vibe_ocr_timeout,cfg.vibe_ocr_max_tokens)?;
    if suffix=="pdf" {
        let file=path.to_owned();
        let cb_for_render=cb.clone();
        let (pages,total)=tokio::task::spawn_blocking(move||pdf_pages_as_jpegs(&file,max_pages,&cb_for_render)).await??;
        let processed=pages.len();
        let mut snippets=Vec::with_capacity(processed);
        let mut truncated=processed<total;
        for (i,page) in pages.iter().enumerate(){
            progress(cb,"api",i as i32,processed as i32,format!("VibeCode: отправка страницы {} из {processed}",i+1));
            // Keep page position even if one page fails: partial OCR is NOT full compliance.
            match recognize_with_heartbeat(&ocr,page,"image/jpeg",cb,i as i32,processed as i32).await {
                Ok(result)=>{
                    truncated |= result.truncated;
                    snippets.push(format!("=== Страница {} ===\n{}",i+1,result.text));
                    progress(cb,"api",(i+1) as i32,processed as i32,format!("Распознана страница {} из {processed}",i+1));
                },
                Err(e)=>{
                    truncated=true;
                    tracing::warn!(page=i+1,"VibeCode OCR failed for PDF page: {e:#}");
                    progress(cb,"page_error",(i+1) as i32,processed as i32,format!("Ошибка страницы {}: {}",i+1,e.to_string().chars().take(200).collect::<String>()));
                    snippets.push(format!("=== Страница {} ===\n[НЕ РАСПОЗНАНО: требуется ручная проверка]",i+1));
                }
            }
        }
        let has_text=snippets.iter().any(|s|!s.contains("[НЕ РАСПОЗНАНО:"));
        return Ok(Extracted{text:snippets.join("\n\n"),status:if !has_text{"manual"}else if truncated{"partial"}else{"ocr_vibe"}.into(),confidence:None});
    }
    // Unlike PDF, image inputs are sent DIRECTLY without conversion, just
    // as in the caller's tested Bitrix VibeCode snippet.
    let mime=image_extension_mime(&suffix)?;
    let file=path.to_owned();
    let (raw,multi_tiff)=tokio::task::spawn_blocking(move||{
        let raw=fs::read(file)?;
        let multi_tiff=if mime=="image/tiff" {tiff_page_count(&raw)!=Some(1)}else{false};
        Ok::<_,anyhow::Error>((raw,multi_tiff))
    }).await??;
    progress(cb,"api",0,1,format!("VibeCode: отправлено изображение ({}, {} байт)",suffix,raw.len()));
    let recognized=recognize_with_heartbeat(&ocr,&raw,mime,cb,0,1).await?;
    progress(cb,"api",1,1,"Ответ VibeCode получен");
    let mut text=recognized.text;
    if multi_tiff {text.push_str("\n[ВНИМАНИЕ: TIFF многостраничный или число страниц неизвестно; полнота OCR требует проверки]");}
    Ok(Extracted{text,status:if recognized.truncated||multi_tiff{"partial"}else{"ocr_vibe"}.into(),confidence:None})
}

pub async fn extract_with_progress(path:&Path,max_pages:usize,cfg:&Config,cb:Option<ProgressCallback>)->Extracted{
    progress(&cb,"start",0,0,format!("Начато распознавание: {}",path.file_name().and_then(|n|n.to_str()).unwrap_or("файл")));
    match run_extract(path,max_pages,cfg,&cb).await {
        Ok(v)=>{
            progress(&cb,"result",1,1,format!("Распознавание завершено, статус {}",v.status));
            v
        },
        Err(e)=>{
            progress(&cb,"error",0,0,format!("Ошибка распознавания: {}",e.to_string().chars().take(220).collect::<String>()));
            Extracted{text:String::new(),status:format!("error: {}",e.to_string().chars().take(180).collect::<String>()),confidence:None}
        }
    }
}

pub async fn extract(path:&Path,max_pages:usize,cfg:&Config)->Extracted{
    extract_with_progress(path,max_pages,cfg,None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn joins_docx_runs_without_fake_spaces(){
        let xml=r#"<w:p><w:r><w:t>Акт </w:t></w:r><w:r><w:t>освидетельствования</w:t></w:r></w:p>"#;
        assert_eq!(docx_xml_to_text(xml).unwrap(),"Акт освидетельствования\n");
    }
    #[test] fn preserves_requisite_across_bold_run(){
        let xml=r#"<w:p><w:r><w:t>ГО</w:t></w:r><w:r><w:t>СТ</w:t></w:r></w:p>"#;
        assert_eq!(docx_xml_to_text(xml).unwrap(),"ГОСТ\n");
    }
    #[test] fn reads_legacy_xls_without_manual_review() {
        let file=Path::new(concat!(env!("CARGO_MANIFEST_DIR"),"/tests/fixtures/legacy_excel.xls"));
        let text=spreadsheet_text(file).unwrap();
        assert!(text.contains("Акт освидетельствования"));
        assert!(text.contains("Сваи"));
    }
    #[tokio::test] async fn converts_legacy_doc_automatically_when_libreoffice_exists() {
        let executable=libreoffice_executable();
        if !std::process::Command::new(&executable).arg("--version").output().is_ok_and(|o|o.status.success()) {
            // CI without LibreOffice can still test XLS and the rest of Rust.
            return;
        }
        let testdir=tempfile::tempdir().unwrap();
        let input=Path::new(concat!(env!("CARGO_MANIFEST_DIR"),"/tests/fixtures/legacy_word.doc"));
        let staged=testdir.path().join("input.doc");
        std::fs::copy(input,&staged).unwrap();
        let output=convert_legacy_doc(&staged,testdir.path(),"docx").await.unwrap();
        let text=docx_text(&output).unwrap();
        assert!(text.contains("Акт освидетельствования"));
    }
    #[test] fn image_formats() {
        assert_eq!(image_extension_mime("tif").unwrap(),"image/tiff");
        assert_eq!(image_extension_mime("bmp").unwrap(),"image/bmp");
        assert_eq!(image_extension_mime("gif").unwrap(),"image/gif");
        assert!(supported(Path::new("example.TIFF")));
        let data=include_bytes!("../tests/fixtures/sample.tif");
        assert_eq!(tiff_page_count(data),Some(1));
        assert!(data.starts_with(b"II") || data.starts_with(b"MM"));
    }
    #[test] fn render_pdf_to_individual_jpeg_pages() {
        let file=Path::new(concat!(env!("CARGO_MANIFEST_DIR"),"/tests/fixtures/test_2_pages.pdf"));
        let (images,total)=pdf_pages_as_jpegs(file,5,&None).unwrap();
        assert_eq!(total,2);
        assert_eq!(images.len(),2);
        for image in images { assert!(image.starts_with(&[0xFF,0xD8,0xFF])); }
    }
    #[test] fn pdf_page_limit_is_reported() {
        let file=Path::new(concat!(env!("CARGO_MANIFEST_DIR"),"/tests/fixtures/test_2_pages.pdf"));
        let (images,total)=pdf_pages_as_jpegs(file,1,&None).unwrap();
        assert_eq!(total,2);
        assert_eq!(images.len(),1);
    }

}
