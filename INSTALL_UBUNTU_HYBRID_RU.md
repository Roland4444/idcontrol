# Установка Контроль ИД v0.6 на Ubuntu 24.04 LTS (без Docker)

Архитектура: Synology DSM HTTPS:5001 (только чтение) → Rust/Axum на Ubuntu → PostgreSQL локально 127.0.0.1:5432 → инженер по внутреннему HTTPS. ИИ выключен по умолчанию, может быть включён через официальный GigaChat API либо через подготовленный компанией gateway для GigaCowork.

## 0. Требования

- Ubuntu Server 24.04 LTS, ориентир для пилота 4 vCPU, 8 ГБ ОЗУ, SSD 100+ ГБ (архив растёт по объёму ИД).
- Доступ сервера к NAS `https://HOST:5001` по локальной сети/VPN.
- Учётная запись NAS с правами **только чтения** конкретной общей папки и вложенных папок, без File Station API на запись.
- Рабочий сертификат TLS NAS, выданный доверенным УЦ, или PEM корневого корпоративного УЦ.
- Для ИИ-режима необходимы исходящие HTTPS-соединения до API Сбера/корпоративного gateway; **входящий публичный IP не нужен**.
- Доступ к веб-интерфейсу только через внутренний HTTPS reverse proxy и/или VPN.

## 1. PostgreSQL (локально на Ubuntu)

```bash
sudo apt update
sudo apt install -y postgresql postgresql-client postgresql-contrib
sudo systemctl enable --now postgresql
sudo systemctl status postgresql --no-pager
```

Создать отдельную роль с вводом пароля интерактивно:

```bash
sudo -u postgres createuser --pwprompt --no-superuser --no-createrole --no-createdb idcontrol
sudo -u postgres createdb --owner=idcontrol --encoding=UTF8 idcontrol
```

Проверка (введите заданный пароль, когда PostgreSQL запросит его):

```bash
psql -h 127.0.0.1 -p 5432 -U idcontrol -d idcontrol -c 'SELECT current_database(), current_user;'
```

Не нужно открывать порт PostgreSQL в интернет и на роутере. На локальной машине применяйте `127.0.0.1:5432`. Если пароль содержит `@`, `:`, `#`, `/`, `%` или другие специальные символы, в `DATABASE_URL` он должен быть URL-кодирован (например `@` → `%40`). Для Ubuntu стандартная установка PostgreSQL обычно уже настроена слушать локальный интерфейс; дополнительно проверьте:

```bash
sudo ss -tlnp | grep 5432
sudo -u postgres psql -c 'SHOW listen_addresses;'
```

Проверьте `pg_hba.conf`: для TCP с `127.0.0.1/32` рекомендуется парольная аутентификация `scram-sha-256`. Не переключайте её на `trust`.

## 2. Rust и системные зависимости (OCR через VibeCode)

```bash
sudo apt install -y build-essential pkg-config libssl-dev ca-certificates curl unzip \
  libreoffice-writer fonts-dejavu-core nginx
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
rustup update stable
rustc --version
cargo --version
```

Установите Rust от имени учётной записи разработчика, а не root, поскольку сборка под root не нужна. Команда rustup скачивает компоненты из интернета и требует исходящего доступа при установке.

## 3. Сборка проекта

Распакуйте архив в домашний каталог пользователя, например:

```bash
unzip Контроль_ИД_Rust_Axum_GigaCowork_Hybrid_v0.6.zip
cd id-control-axum
cargo fmt --all -- --check
cargo check
cargo test
cargo build --release
```

Если `cargo` выдаёт ошибку, сначала исправьте её. Не запускайте systemd с несобранным бинарником.

## 4. Проверка папки Synology

В File Station узнайте точный путь от имени общей папки, например:

```text
/shared_docs/Исполнительная документация
```

Это путь `SYNOLOGY_ROOT`, а НЕ `/volume1/shared_docs/...`. Для распознавания и автоматического назначения проекта в каждой папке комплектов сейчас используется структура:

```text
/shared_docs/Исполнительная документация/
└── ЖК Северный/
    └── Земляные работы/
        ├── ID/
        │   ├── АОСР-15.docx
        │   └── Реестр.xlsx
        └── REFERENCE/
            └── РД.pdf
```

Папка `ID` также может называться `ИД`; `REFERENCE` — `ПД-РД`. Также поддерживается структура, в которой `ПД` и `ИД` лежат непосредственно внутри `SYNOLOGY_ROOT` (см. раздел v0.6.1 ниже). Файлы вне распознанных структур игнорируются.

Укажите IP/DNS NAS и HTTPS-порт DSM:

```dotenv
SYNOLOGY_URL=https://192.168.1.50:5001
SYNOLOGY_ROOT=/shared_docs/Исполнительная документация
SYNOLOGY_USERNAME=id_reader
SYNOLOGY_PASSWORD=ваш_пароль
SYNOLOGY_VERIFY_TLS=true
```

Если сертификат издан собственным УЦ, добавьте доверенный PEM:

```dotenv
SYNOLOGY_CA_CERT=/etc/id-control-axum/nas-ca.pem
```

Настройка `SYNOLOGY_VERIFY_TLS=false` не рекомендуется. При обращении по IP сертификат должен содержать соответствующий IP в SAN; иначе используйте DNS-имя из сертификата.

## 5. Установка исполняемого файла и настроек

```bash
sudo useradd --system --user-group --home-dir /var/lib/id-control \
  --create-home --shell /usr/sbin/nologin idcontrol
sudo install -d -o idcontrol -g idcontrol -m 700 /var/lib/id-control
sudo install -d -o root -g root -m 755 /opt/id-control-axum
sudo install -o root -g root -m 755 target/release/id-control-axum \
  /opt/id-control-axum/id-control-axum
sudo cp -r catalog /opt/id-control-axum/
sudo cp -r migrations /opt/id-control-axum/
sudo install -o root -g idcontrol -m 640 .env.example /etc/id-control-axum.env
sudo nano /etc/id-control-axum.env
```

В файле `/etc/id-control-axum.env` минимум заполните:

```dotenv
DATABASE_URL=postgresql://idcontrol:DB_PASSWORD@127.0.0.1:5432/idcontrol
LISTEN=127.0.0.1:8090
APP_USER=admin
APP_PASSWORD=REPLACE_WITH_YOUR_PASSWORD
SYNOLOGY_URL=https://nas.company.local:5001
SYNOLOGY_USERNAME=id_reader
SYNOLOGY_PASSWORD=NAS_PASSWORD
SYNOLOGY_ROOT=/shared_docs/Исполнительная документация
SYNOLOGY_VERIFY_TLS=true
DATA_ROOT=/var/lib/id-control
POLL_INTERVAL=45
STABLE_SECONDS=15
AI_MODE=off
```

Не помещайте реальные пароли в исходный код или Git. Ограничьте чтение файла конфигурации пользователями root и idcontrol.

## 6. Миграции PostgreSQL

Проект использует **версионированные миграции SQLx** из каталога `migrations/`:

- `001_init.sql`: проекты, комплекты, документы и версии, проверки, замечания, аудит.
- `002_norms.sql`: утверждаемые правила нормативного контроля и результаты.
- `003_norm_patterns.sql`: проверка реквизитов регулярными выражениями.
- `004_synology_index.sql`: кеш метаданных файлов DSM.
- `005_ai_hybrid.sql`: согласие на ИИ по комплекту, очередь ИИ-заданий, черновики предложений ИИ.

SQLx создаёт `_sqlx_migrations`, хранит выполненные версии и контрольные суммы. Новые миграции добавляйте отдельными следующими файлами, **не изменяйте уже применённые**.

В файле `deploy/id-control-axum.service` указано `ExecStartPre=... --migrate-only`: перед каждым стартом автоматически применяется только то, что ещё не было применено. Если миграция не прошла, основной процесс не стартует.

Проверка после запуска:

```bash
psql -h 127.0.0.1 -U idcontrol -d idcontrol -c 'SELECT version, description, success FROM _sqlx_migrations ORDER BY version;'
psql -h 127.0.0.1 -U idcontrol -d idcontrol -c '\dt'
```

**Обновление с v0.5:** сначала сделайте резервную копию БД, остановите сервис, поставьте новый бинарник и новую systemd-конфигурацию, затем запустите. Старые DDL (`001`–`004`) в v0.5 применялись без таблицы SQLx; их повторное исполнение в этой версии задумано идемпотентным. Перед обновлением производственной БД всё равно проверьте это на копии — не проводите первое обновление на единственном экземпляре.

## 7. Служба systemd

```bash
sudo cp deploy/id-control-axum.service /etc/systemd/system/id-control-axum.service
sudo systemctl daemon-reload
sudo systemctl enable --now id-control-axum
sudo systemctl status id-control-axum --no-pager
sudo journalctl -u id-control-axum -n 120 --no-pager
```

Проверка приложения:

```bash
curl -u admin http://127.0.0.1:8090/health
```

Пароль будет запрошен интерактивно. Для проверок доступа к NAS контролируйте `/health` и логи `journalctl`. Приложение при старте периодически сканирует настроенную папку Synology.

## 8. Внутренний HTTPS (Nginx)

В каталоге `deploy/nginx.conf` приведён образец. Измените `server_name` на внутренний DNS и укажите действующие сертификат/ключ. Затем:

```bash
sudo cp deploy/nginx.conf /etc/nginx/sites-available/id-control
sudo ln -s /etc/nginx/sites-available/id-control /etc/nginx/sites-enabled/id-control
sudo nginx -t
sudo systemctl reload nginx
```

Убедитесь, что порт 443 доступен только пользователям корпоративной сети или VPN. Внешний IP для веб-приложения не требуется. Порт DSM 5001 тоже не нужно публиковать наружу ради интеграции с приложением.

## 9. Включение гибридного ИИ

**Вариант А. Проверка только локально:** `AI_MODE=off` — самый безопасный запуск пилота.

**Вариант Б. Через GigaChat API:**

```dotenv
AI_MODE=gigachat
GIGACHAT_AUTH_KEY=ВАШ_КЛЮЧ_АВТОРИЗАЦИИ
GIGACHAT_SCOPE=GIGACHAT_API_B2B
GIGACHAT_MODEL=GigaChat-2-Pro
AI_MAX_DOCS=6
AI_MAX_CHARS_PER_DOC=4000
```

Получите ключ в кабинете Сбера; проверьте доступность модели по вашему тарифу. Приложение автоматически получает временный OAuth-токен, затем обращается к GigaChat API. При необходимости установите `AI_CA_CERT` с корпоративным доверенным УЦ. Передача фрагментов наружу требует разрешения в интерфейсе **для каждого комплекта отдельно**, после оценки содержания и правомерности передачи.

**Вариант В. Возможная интеграция через GigaCowork:**

```dotenv
AI_MODE=cowork_gateway
AI_GATEWAY_URL=https://ai-gateway.company.local/v1/document-review
AI_GATEWAY_TOKEN=TOKEN_FOR_YOUR_OWN_GATEWAY
```

**Такой шлюз не поставляется и прямой API GigaCowork не подтверждён.** Это наш подготовленный контракт для будущей реализации адаптера после получения документации и разрешения от владельца платформы. См. `docs/COWORK_GATEWAY_CONTRACT.md`. Использовать режим с адресом самого веб-сайта GigaCowork нельзя.

После изменения `.env`:

```bash
sudo systemctl restart id-control-axum
```

В веб-интерфейсе откройте комплект, включите разрешение и нажмите «Запустить ИИ-проверку». Статусы заданий отображаются на странице. Ответ ИИ — только черновой список для подтверждения инженером.

## 10. Резервное копирование и восстановление

Создание бэкапа PostgreSQL:

```bash
pg_dump -h 127.0.0.1 -U idcontrol -d idcontrol -Fc -f idcontrol-$(date +%F).dump
```

Архив оригиналов (`/var/lib/id-control`) также резервируйте отдельно, потому что PostgreSQL хранит ссылки на файлы. Проверьте процедуру восстановления на тестовом сервере; не ограничивайтесь фактом создания архива. Не копируйте секреты в бэкап, доступный всем пользователям.

## 11. Типичные неисправности

- `password authentication failed`: неверный пароль PostgreSQL, URL-кодирование или `pg_hba.conf`.
- `relation ... does not exist`: миграции не применились, смотрите `systemctl status` и `journalctl`.
- Ошибка DSM 401/403 или код API: у пользователя нет прав/неверная папка/учётная запись заблокирована.
- Ошибка TLS: проверяйте DNS имя и SAN сертификата; при собственном УЦ добавьте его PEM.
- Ничего не найдено на NAS: проверьте `SYNOLOGY_ROOT` и структуру `Объект/Вид работ/ID|REFERENCE`.
- ИИ не запускается: `AI_MODE=off`, нет `ai_consent`, нет эталонного документа или не распознан текст.
- Ошибка ИИ OAuth 401: проверьте ключ, scope и доступ к GigaChat API. Ошибка 404: проверьте ID модели и тариф.
- Ошибка PDF: используется Hayro (чистый Rust, без Poppler); при неподдерживаемом PDF требуется ручная проверка, а не выдача ложного соответствия.
- Ошибка OCR HTTP 401/403: проверьте VIBE_OCR_KEY и права доступа к VibeCode; HTTP 429: лимит провайдера. Оригиналы не меняются.

## Ограничения пилота

Веб-приложение использует одну Basic Auth учётную запись и не реализует полноценный корпоративный RBAC. Это исходники пилотной версии, а не сертифицированная система нормативного контроля. Не используйте отсутствие найденных замечаний как официальное подтверждение соответствия ГОСТ/СП. Полная сверка DWG/DXF и геометрии ИС/РД пока не реализована.


## Вариант для Synology с папками ПД/ и ИД/ непосредственно под корнем (v0.6.1)

Если физический путь `/volume1/ResData/ИТ/test/тест (esinitsin)`, в File Station API:

```dotenv
SYNOLOGY_ROOT="/ResData/ИТ/test/тест (esinitsin)"
SYNOLOGY_PROJECT_NAME="тест (esinitsin)"
SYNOLOGY_DEFAULT_WORK="Вид работ не указан"
```

Структура `/ResData/ИТ/test/тест (esinitsin)/ПД/*.pdf` и `/ИД/*.docx`
теперь поддерживается без дополнительного вложенного каталога проекта/вида работ.
Вариант для действующей старой вложенной структуры тоже сохранён.

Для просмотра того, что NAS API возвращает без записи в БД, выполните:
`cargo run -- --list-synology`. Для логов используйте `RUST_LOG=id_control_axum=info`.
Если в этой диагностике `0 files`, проверьте права DSM и точность SYNOLOGY_ROOT.
Если файлы перечислены, но сканер не импортирует их, посмотрите строку `DSM scan detail`.


## Обновление v0.6.3: VibeCode OCR

`pdftoppm`, `pdftotext`, `tesseract` больше не требуются. Для работы PDF требуется современный Rust >= 1.92 при сборке (`hayro` 0.8); сам PDF нельзя отправить напрямую в VibeCode, он разделяется на изображения страниц. TIFF/BMP/GIF отправляются напрямую, как в проверенном тесте. Добавить в `.env` параметры из `docs/VIBECODE_OCR_RU.md`. Затем `cargo build --release && ./target/release/id-control-axum --reprocess-errors`. Включение VIBE_OCR_ENABLED=true разрешает отправку изображений документов внешнему API.
