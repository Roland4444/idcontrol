# Исправление сборки v0.6.5 (hayro 0.8.0)

Причина: в hayro 0.8.0 параметры x_scale/y_scale находятся в `PixmapSettings`, а не `RenderSettings`. В `render` требуется 5-й аргумент `&PixmapSettings`.

Изменения:
- `src/docs.rs`: импорт PixmapSettings, разделены настройки рендеринга и пиксельной карты, вызов render с пятью аргументами.
- Убраны неиспользуемые импорты в `src/docs.rs` и `src/reprocess.rs`.
- Неиспользуемый результат INSERT в `src/scanner.rs` переименован в `_rec`.
- `Cargo.toml`: версия проекта 0.6.5, `hayro = "0.8"` остаётся.
- Сохранена заданная пользователем строка `if password.len()<3 || password.starts_with("REPLACE") || password.starts_with("CHANGE") {`.
- Структура PostgreSQL не меняется, дополнительных SQL-миграций нет.

Сборка и проверка на Windows:
```powershell
cargo check
cargo test
cargo run -- --check-vibe-ocr "test.pdf"
cargo run
```

Это исходники, собранные и проверенные статически: в среде упаковки отсутствует Rust toolchain, поэтому фактическую компиляцию на стороне автора архива выполнить не удалось.
