**Обновление v0.6.9**: OCR продолжает работать локально, классификация — отдельной языковой моделью. В интерфейсе комплекта выбирается VibeCode или GigaCowork (последний только через ваш адаптер). Для начала требуется согласие комплекта на внешний ИИ, выбранный провайдер и сохранённый в `.env` ключ. Новая миграция 007. Подробности: `docs/GIGACOWORK_CLASSIFIER_ADAPTER_RU.md`. **Каталог `models/` в архив не включён.**

**Windows v0.6.8:** локальные пути `DATA_ROOT=./storage`, `TEMP_ROOT=./storage/temp`, `LOCAL_OCR_MODEL_DIR=./models/ocr` (от папки `.env`). Важное предупреждение о старых архивных файлах: [WINDOWS_LOCAL_STORAGE_V068_RU.md](WINDOWS_LOCAL_STORAGE_V068_RU.md). Версия `Cargo.toml` теперь корректная `0.6.8`.

**v0.6.7.5:** добавьте в `.env` `OCR_PAGE_WORKERS=6` (для CPU из 6 ядер можно начать с 3) и `OCR_MAX_PAGES=500`; перезапустите сервер. Текстовый порядок страниц сохраняется, прогресс доступен на `/diagnostics`. Новых SQL-миграций нет.

**Важное обновление v0.6.7:** перед первым запуском OCR установить локальные ONNX-модели через `powershell -ExecutionPolicy Bypass -File .\scripts\install-local-ocr-models.ps1` и задать `OCR_MODE=local`; см. docs/LOCAL_NEURAL_OCR_RU.md.

# Быстрый запуск v0.6.4 в Windows + PostgreSQL на WSL2

1. **Сохрани свой `.env` из старого проекта**. Не замещай его новым `.env.example` — это шаблон, не рабочая конфигурация.
2. Распакуй архив. Открой в VS Code **папку `id-control-axum`**, которая содержит `Cargo.toml`.
3. Скопируй свой `.env` в корень новой версии. Допиши параметры OCR из `docs/VIBECODE_OCR_RU.md`.
4. Проверь параметры `DATABASE_URL` (PostgreSQL в WSL2), `SYNOLOGY_URL=https://...:5001`, `SYNOLOGY_ROOT`.
5. Выполни команды из терминала PowerShell:

```powershell
cargo check
cargo test
cargo run -- --check-vibe-ocr "C:\\path\\to\\00.tif"
cargo run -- --check-vibe-ocr "C:\\path\\to\\test.pdf"
cargo run -- --migrate-only
cargo run -- --reprocess-errors
cargo run
```

**Не запускай `cargo run -- --reprocess-errors` одновременно с уже запущенным процессом сервера.** Останови сервер через `Ctrl+C` перед повторной обработкой.

Изображения TIFF/BMP/GIF/JPG/PNG передаются непосредственно в VibeCode. **PDF должен быть отрендерен по страницам**, как реализовано в `src/docs.rs` с помощью Rust-библиотеки `hayro`. Ключ берётся из `VIBE_OCR_KEY` или `VIBE_OCR_KEY_FILE`. При PDF на 20 страниц выполняется до 20 отдельных запросов.

Старые таблицы PostgreSQL не удаляются. Все 5 SQL-миграций сохранены, а SQLx применит только ещё не применённые.

При тестах с Windows/WSL2 и Synology сертификат TLS NAS может требовать настройки доверия. Отключение проверки TLS допустимо только для кратковременной диагностики в доверенной сети; в постоянной работе используй правильный сертификат и домен NAS.

Пароль приложения: **оставлено минимальное ограничение 3 байта**, строка `if password.len()<3 || password.starts_with("REPLACE") || password.starts_with("CHANGE") {` сохранена.


### v0.6.6: диагностика и кнопки OCR

После `cargo run` откройте `/diagnostics`; на странице комплекта доступны «Повторить OCR» и «Распознать все с ошибками». Новая миграция 006 применяется автоматически. Подробности: `OCR_DIAGNOSTICS_V066_RU.md`.
