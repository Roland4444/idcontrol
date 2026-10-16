# Исправление PowerShell-установщика — v0.6.7.1

Причина: `install-local-ocr-models.ps1` из v0.6.7 был в UTF-8 без BOM и содержал русские строки.
Windows PowerShell 5.1 часто декодирует такой файл с системной кодировкой (Windows-1251), что приводило
к ошибке `TerminatorExpectedAtEndOfString` на строке с `Write-Host`.

Исправления:
- UTF-8 BOM в начале скрипта для совместимости с Windows PowerShell 5.1;
- служебные сообщения в PowerShell-скрипте на ASCII; содержимое и URL моделей не менялись;
- исходники Rust, миграции, параметры `APP_PASSWORD` и база данных не изменены.

Выполни в корне проекта:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\install-local-ocr-models.ps1
cargo run -- --check-local-models
```

Должны появиться `models/ocr/det.onnx`, `models/ocr/rec.onnx`, `models/ocr/dict.txt`.
Загрузка моделей требует доступ к huggingface.co. При ошибке SHA-256 проверка остановит
установку, а не примет неподтверждённый файл. Модели не вложены в ZIP.

Альтернатива для старой папки проекта — замени только файл `scripts/install-local-ocr-models.ps1`.
Если приложение уже работает и менялся только установщик, Rust перекомпилировать не требуется.

Проверка выполнена для байтов BOM, списка файлов и целостности архива.
Фактическая загрузка моделей и выполнение PowerShell не проверялись в данной среде.
