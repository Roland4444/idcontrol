//! Local neural OCR: ONNX text detector + Cyrillic recognizer.
//! Pages are processed in parallel by a bounded pool of native threads.
//! Each worker owns one ONNX engine (no concurrent access to an engine).
//! The producer/consumer queue is bounded to limit PDF image memory.
use anyhow::{anyhow, bail, Context, Result};
use rapidocr_core::{RapidOcr, config::{RapidOcrConfig, PipelineConfig}};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, mpsc, atomic::{AtomicBool, AtomicUsize, Ordering}},
    time::Instant,
};
use crate::docs::{OcrProgress, ProgressCallback};

pub const DET_MODEL: &str = "det.onnx";
pub const REC_MODEL: &str = "rec.onnx";
pub const REC_DICT: &str = "dict.txt";

pub fn model_files(dir: &Path) -> [PathBuf; 3] {
    [dir.join(DET_MODEL), dir.join(REC_MODEL), dir.join(REC_DICT)]
}

pub fn ensure_models(dir: &Path) -> Result<()> {
    for (i, file) in model_files(dir).iter().enumerate() {
        let len = std::fs::metadata(file)
            .with_context(||format!("Нет локальной OCR-модели {}. Запусти scripts/install-local-ocr-models.ps1 или .sh", file.display()))?.len();
        let minimum = if i == 2 { 100 } else { 1_000_000 };
        if len < minimum {
            bail!("OCR-модель {} слишком мала ({len} байт); проверь скачивание ONNX", file.display());
        }
    }
    Ok(())
}

fn progress(cb: &Option<ProgressCallback>, stage: &'static str, current: i32, total: i32, message: impl Into<String>) {
    if let Some(cb) = cb { cb(OcrProgress { stage, current, total, message: message.into() }); }
}

/// One private inference session pair per worker. Never share a mutable engine
/// across threads, and never reload it for each page.
fn runner(dir: &Path) -> Result<RapidOcr> {
    ensure_models(dir)?;
    let mut settings = RapidOcrConfig::ppocr_v6_small(dir);
    settings.pipeline = PipelineConfig::without_cls();
    settings.cls = None;
    settings.det.as_mut().ok_or_else(||anyhow!("OCR detector configuration missing"))?
        .model_path = dir.join(DET_MODEL);
    let rec = settings.rec.as_mut().ok_or_else(||anyhow!("OCR recognizer configuration missing"))?;
    rec.model_path = dir.join(REC_MODEL);
    rec.dict_path = dir.join(REC_DICT);
    // PP-OCRv5 Cyrillic model expects [channels,height,max_width] = [3,48,320].
    rec.image_shape = [3, 48, 320];
    rec.batch_size = 4;
    settings.text_score = 0.35;
    RapidOcr::new(settings).context("Не удалось запустить ONNX OCR (модели или ONNX Runtime)")
}

#[derive(Debug)]
pub struct PageText {
    pub text: String,
    pub confidence: Option<f32>,
}

fn recognize_one(engine: &mut RapidOcr, bytes: &[u8], page_index: usize) -> Result<PageText> {
    let page = page_index + 1;
    let image = image::load_from_memory(bytes)
        .with_context(||format!("Не удалось открыть изображение страницы {page}"))?.to_rgb8();
    let output = engine.run_image(&image)
        .with_context(||format!("Сбой ONNX на странице {page}"))?;
    let lines: Vec<_> = output.lines.into_iter().filter(|line|!line.text.trim().is_empty()).collect();
    let confidence = if lines.is_empty() { None } else {
        Some(lines.iter().map(|line|line.score).sum::<f32>() / lines.len() as f32)
    };
    Ok(PageText {
        text: lines.iter().map(|line|line.text.as_str()).collect::<Vec<_>>().join("\n"),
        confidence,
    })
}

/// Stream rendered pages into a bounded queue while OCR workers process them.
/// The producer runs on the calling thread. Each worker loads its ONNX models
/// ONCE and recognizes many pages. The result always uses original page order.
/// Call this from tokio::task::spawn_blocking, not from an async executor thread.
/// `produce` must submit each index in [0,total) exactly once.
pub fn recognize_stream<F>(
    total: usize,
    model_dir: &Path,
    requested_workers: usize,
    cb: Option<ProgressCallback>,
    produce: F,
) -> Result<Vec<PageText>>
where
    F: FnOnce(&mut dyn FnMut(usize, Vec<u8>) -> Result<()>) -> Result<()>,
{
    if total == 0 { bail!("OCR: нет страниц для распознавания"); }
    let workers = worker_count(requested_workers, total);
    let started = Instant::now();
    progress(&cb, "local_workers", 0, total as i32,
        format!("Запуск {workers} параллельных OCR-обработчиков; очередь до {} страниц", workers * 2));

    let (input_tx, input_rx) = mpsc::sync_channel::<(usize, Vec<u8>)>(workers * 2);
    let queue = Arc::new(Mutex::new(input_rx));
    let (result_tx, result_rx) = mpsc::channel::<(usize, Result<PageText>)>();
    let completed = Arc::new(AtomicUsize::new(0));
    let stopped = Arc::new(AtomicBool::new(false));
    let mut submitted = 0usize;

    let produced = std::thread::scope(|scope| -> Result<()> {
        let mut handles = Vec::with_capacity(workers);
        for worker in 0..workers {
            let queue = Arc::clone(&queue);
            let sender = result_tx.clone();
            let count = Arc::clone(&completed);
            let stopped = Arc::clone(&stopped);
            let dir = model_dir.to_path_buf();
            let callback = cb.clone();
            handles.push(scope.spawn(move || -> Result<()> {
                let loaded_at = Instant::now();
                let mut engine = runner(&dir).with_context(||format!("OCR worker {}: ошибка загрузки моделей", worker + 1))?;
                progress(&callback, "local_workers", count.load(Ordering::SeqCst) as i32, total as i32,
                    format!("OCR worker {}/{} готов, ONNX загружен за {:.1} с", worker + 1, workers, loaded_at.elapsed().as_secs_f32()));
                loop {
                    if stopped.load(Ordering::SeqCst) { break; }
                    let next = queue.lock().map_err(|_|anyhow!("OCR input queue poisoned"))?.recv();
                    let Ok((index, bytes)) = next else { break; };
                    if stopped.load(Ordering::SeqCst) { break; }
                    progress(&callback, "local_page_start", count.load(Ordering::SeqCst) as i32, total as i32,
                        format!("Поток {}: распознаю страницу {} из {total}", worker + 1, index + 1));
                    let result = recognize_one(&mut engine, &bytes, index);
                    match &result {
                        Ok(page) => {
                            let n = count.fetch_add(1, Ordering::SeqCst) + 1;
                            let rate = n as f64 * 60.0 / started.elapsed().as_secs_f64().max(0.01);
                            progress(&callback, "local_page", n as i32, total as i32,
                                format!("Готово {n}/{total} страниц, последняя №{}, {} символов, {:.1} стр./мин, поток {}",
                                    index + 1, page.text.chars().count(), rate, worker + 1));
                        }
                        Err(e) => {
                            stopped.store(true, Ordering::SeqCst);
                            progress(&callback, "page_error", count.load(Ordering::SeqCst) as i32, total as i32,
                                format!("Ошибка страницы {}: {e:#}", index + 1));
                        }
                    }
                    if sender.send((index, result)).is_err() { break; }
                }
                Ok(())
            }));
        }
        // Do not retain an extra receiver after workers exit: otherwise a
        // blocked producer cannot detect that no consumers remain.
        drop(queue);
        drop(result_tx);
        let mut last_index = None;
        let mut enqueue = |index: usize, bytes: Vec<u8>| -> Result<()> {
            if index >= total { bail!("OCR page index {index} exceeds total {total}"); }
            if last_index.map(|last|index != last + 1).unwrap_or(index != 0) {
                bail!("OCR page sequence mismatch at index {index}");
            }
            if stopped.load(Ordering::SeqCst) { bail!("OCR stopped after a worker error"); }
            input_tx.send((index, bytes)).map_err(|_|anyhow!("OCR workers stopped before all pages were queued"))?;
            last_index = Some(index);
            submitted += 1;
            Ok(())
        };
        let producer_result = produce(&mut enqueue);
        drop(enqueue);
        drop(input_tx);
        let mut worker_error = None;
        for handle in handles {
            match handle.join() {
                Ok(Ok(())) => {},
                Ok(Err(e)) => if worker_error.is_none() {worker_error = Some(e)},
                Err(_) => if worker_error.is_none() {worker_error = Some(anyhow!("OCR worker panicked"))},
            }
        }
        // Prefer ONNX initialization errors over the resulting queue disconnect.
        if let Some(e) = worker_error { return Err(e); }
        producer_result?;
        if submitted != total { bail!("OCR: подготовлено {submitted} страниц вместо {total}"); }
        Ok(())
    });

    // Receives text in completion order, then reconstructs original page order.
    let mut ordered: Vec<Option<PageText>> = (0..total).map(|_|None).collect();
    let mut first_error = None;
    for (index, result) in result_rx {
        match result {
            Ok(page) => ordered[index] = Some(page),
            Err(e) => if first_error.is_none() { first_error = Some(e) },
        }
    }
    // A model error from a page is more informative than a queue disconnect.
    if let Some(e) = first_error { return Err(e); }
    produced?;
    if ordered.iter().any(|page|page.is_none()) {
        bail!("OCR обработал {} из {total} страниц, результат неполный", completed.load(Ordering::SeqCst));
    }
    progress(&cb, "local_done", total as i32, total as i32,
        format!("OCR завершён за {:.1} с ({:.1} страниц/мин, {workers} потоков)",
            started.elapsed().as_secs_f64(), total as f64 * 60.0 / started.elapsed().as_secs_f64().max(0.01)));
    Ok(ordered.into_iter().map(Option::unwrap).collect())
}

pub fn worker_count(requested: usize, total: usize) -> usize {
    requested.clamp(1, 16).min(total.max(1))
}

pub fn recognize_pages(images: Vec<Vec<u8>>, dir: PathBuf, workers: usize, cb: Option<ProgressCallback>) -> Result<Vec<PageText>> {
    let total = images.len();
    recognize_stream(total, &dir, workers, cb, |emit| {
        for (index, image) in images.into_iter().enumerate() { emit(index, image)?; }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn filenames_are_stable() {
        let p = model_files(Path::new("models/ocr"));
        assert!(p[0].ends_with("det.onnx"));
        assert!(p[1].ends_with("rec.onnx"));
        assert!(p[2].ends_with("dict.txt"));
    }
    #[test]
    fn worker_limit() {
        assert_eq!(worker_count(6, 20), 6);
        assert_eq!(worker_count(6, 2), 2);
        assert_eq!(worker_count(0, 20), 1);
        assert_eq!(worker_count(50, 20), 16);
    }
    #[test]
    fn missing_models_have_clear_error() {
        let dir = std::env::temp_dir().join("__idcontrol_missing_model_dir__");
        assert!(ensure_models(&dir).is_err());
    }
}
