use anyhow::{bail, Context, Result};
use std::{env, path::{Path, PathBuf}};

#[derive(Clone, Debug)]
pub struct Config {
    pub database_url: String,
    pub listen: String,
    pub synology_url: String,
    pub synology_root: String,
    pub synology_project_name: String,
    pub synology_default_work: String,
    pub synology_username: String,
    pub synology_password: String,
    pub synology_verify_tls: bool,
    pub synology_ca_cert: Option<PathBuf>,
    pub data_root: PathBuf,
    pub temp_root: PathBuf,
    pub app_user: String,
    pub app_password: String,
    pub max_file_mb: u64,
    pub ocr_max_pages: usize,
    pub ocr_mode: String,
    pub local_ocr_model_dir: PathBuf,
    pub ocr_page_workers: usize,
    pub vibe_ocr_enabled: bool,
    pub vibe_ocr_url: String,
    pub vibe_ocr_key: Option<String>,
    pub vibe_ocr_model: String,
    pub vibe_ocr_max_tokens: u32,
    pub vibe_ocr_timeout: u64,
    // AI is disabled by default. GigaCowork is NOT assumed to have a public API.
    pub ai_mode: String,
    pub ai_gateway_url: Option<String>,
    pub ai_gateway_token: Option<String>,
    pub ai_gigachat_auth_key: Option<String>,
    pub ai_gigachat_scope: String,
    pub ai_gigachat_model: String,
    pub ai_ca_cert: Option<PathBuf>,
    pub classify_vibe_key: Option<String>,
    pub classify_vibe_model: String,
    pub classify_cowork_url: Option<String>,
    pub classify_cowork_token: Option<String>,
    pub classify_timeout: u64,
    pub classify_max_chars: usize,
    pub ai_max_docs: usize,
    pub ai_max_chars_per_doc: usize,
}
fn var(name: &str, default: &str) -> String { env::var(name).unwrap_or_else(|_| default.into()) }
// Relative paths are resolved against the directory containing .env, not the
// process working directory. This makes Windows VS Code/cargo and a launched
// executable behave consistently even when the current directory changes.
fn resolve_path(project_root:&Path, configured:&str) -> PathBuf {
    let path=PathBuf::from(configured);
    if path.is_absolute() {path} else {project_root.join(path)}
}
fn ensure_local_dir(project_root:&Path, key:&str, configured:&str) -> Result<PathBuf> {
    // On Windows a Linux path like /var/lib/id-control otherwise resolves to
    // the current drive's root (for example C:\\var\\lib...), silently
    // putting archives outside the project. Reject it with an actionable error.
    if cfg!(windows) && (configured=="/var" || configured.starts_with("/var/") ||
        configured=="/tmp" || configured.starts_with("/tmp/")) {
        bail!("{key}={configured} is a Linux path. On Windows set DATA_ROOT=./storage and TEMP_ROOT=./storage/temp in .env. If this path previously stored documents, migrate the archived files and archive_path records before switching.");
    }
    let path=resolve_path(project_root,configured);
    std::fs::create_dir_all(&path).with_context(||format!("Cannot create {key}: {}",path.display()))?;
    path.canonicalize().with_context(||format!("Cannot resolve {key}: {}",path.display()))
}
fn boolean(name: &str, default: bool) -> bool {
    matches!(var(name, if default {"true"} else {"false"}).to_lowercase().as_str(), "true" | "1" | "yes" | "on")
}
impl Config {
    pub fn load() -> Result<Self> {
        // dotenvy searches the current directory and parents; use the actual
        // .env directory as the stable base for all relative local paths.
        let dotenv_path=dotenvy::dotenv().ok();
        let cwd=env::current_dir().context("Cannot detect project directory")?;
        let project_root=match dotenv_path.as_deref().and_then(Path::parent) {
            Some(root) if root.is_absolute()=>root.to_path_buf(),
            Some(root)=>cwd.join(root),
            None=>cwd,
        }.canonicalize().context("Cannot resolve directory containing .env")?;
        let password=var("APP_PASSWORD", "");
        if !std::env::args().any(|arg|arg=="--debug-login") {
        if password.len()<3 || password.starts_with("REPLACE") || password.starts_with("CHANGE") {
            bail!("APP_PASSWORD must have at least 3 bytes and must not start with REPLACE or CHANGE");
        }
        }
        let database_url=var("DATABASE_URL", "");
        if !database_url.starts_with("postgres://") && !database_url.starts_with("postgresql://") {
            bail!("DATABASE_URL must point to PostgreSQL");
        }
        let synology_url=var("SYNOLOGY_URL", "");
        let root=var("SYNOLOGY_ROOT", "");
        if !root.starts_with('/') || root.len()<2 || root.ends_with('/') ||
           root.split('/').any(|c|c=="."||c=="..") || root.contains('\\') ||
           root.chars().any(|c|c.is_control()) {
            bail!("SYNOLOGY_ROOT must be an absolute File Station path, e.g. /resdata/ИД");
        }
        // For a flat NAS root containing just ПД/ and ИД/, derive the project
        // name from the last folder component unless explicitly overridden.
        let default_project=root.rsplit('/').next().unwrap_or("Проект");
        let project_name=var("SYNOLOGY_PROJECT_NAME", default_project);
        let work_name=var("SYNOLOGY_DEFAULT_WORK", "Вид работ не указан");
        for (key,value) in [("SYNOLOGY_PROJECT_NAME",&project_name), ("SYNOLOGY_DEFAULT_WORK",&work_name)] {
            crate::repo::validate_segment(value).with_context(||format!("Invalid {key}"))?;
        }
        let synology_username=var("SYNOLOGY_USERNAME", "");
        let synology_password=var("SYNOLOGY_PASSWORD", "");
        if synology_username.is_empty() || synology_password.is_empty() || synology_password.starts_with("REPLACE") {
            bail!("SYNOLOGY_USERNAME / SYNOLOGY_PASSWORD are required");
        }
        let data_root=ensure_local_dir(&project_root,"DATA_ROOT", &var("DATA_ROOT", "./storage"))?;
        let temp_root=ensure_local_dir(&project_root,"TEMP_ROOT", &var("TEMP_ROOT", "./storage/temp"))?;
        std::fs::create_dir_all(data_root.join("inbox"))?;
        std::fs::create_dir_all(data_root.join("originals"))?;
        let ai_mode=var("AI_MODE", "off");
        if !["off", "gigachat", "cowork_gateway"].contains(&ai_mode.as_str()) {
            bail!("AI_MODE must be off, gigachat or cowork_gateway");
        }
        let gateway=env::var("AI_GATEWAY_URL").ok().filter(|s|!s.is_empty());
        if ai_mode=="cowork_gateway" {
            let url=::url::Url::parse(gateway.as_deref().unwrap_or(""))?;
            if url.scheme()!="https" && !(url.scheme()=="http" && matches!(url.host_str(),Some("127.0.0.1"|"localhost"))) {
                bail!("AI_GATEWAY_URL must use HTTPS (or localhost HTTP)");
            }
            if url.username()!="" || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
                bail!("AI_GATEWAY_URL must not embed credentials or query parameters");
            }
        }
        let ai_key=env::var("GIGACHAT_AUTH_KEY").ok().filter(|s|!s.trim().is_empty());
        if ai_mode=="gigachat" && ai_key.is_none() {
            bail!("Set GIGACHAT_AUTH_KEY to enable GigaChat API");
        }
        let ocr_mode=var("OCR_MODE","local").to_ascii_lowercase();
        if !["local","vibecode","local_then_vibecode"].contains(&ocr_mode.as_str()) {
            bail!("OCR_MODE must be local, vibecode or local_then_vibecode");
        }
        let local_ocr_model_dir=resolve_path(&project_root,&var("LOCAL_OCR_MODEL_DIR","./models/ocr"));
        let vibe_ocr_enabled=boolean("VIBE_OCR_ENABLED",false);
        let vibe_ocr_key=env::var("VIBE_OCR_KEY").ok().filter(|s|!s.trim().is_empty() && !s.starts_with("REPLACE") && !s.starts_with("CHANGE"))
            .or_else(||env::var("VIBE_OCR_KEY_FILE").ok().and_then(|p|{
                std::fs::read_to_string(p).ok().and_then(|s|s.lines().next().map(|s|s.trim().to_string()))
            }));
        if (ocr_mode=="vibecode" || ocr_mode=="local_then_vibecode") && vibe_ocr_enabled && vibe_ocr_key.as_deref().unwrap_or("").is_empty() {
            bail!("VIBE_OCR_ENABLED=true requires VIBE_OCR_KEY or VIBE_OCR_KEY_FILE");
        }
        // Cowork classifier is a user-provided enterprise adapter, not a public endpoint.
        let classify_cowork_url=env::var("CLASSIFY_COWORK_URL").ok().filter(|s|!s.trim().is_empty());
        if let Some(raw)=&classify_cowork_url {
            let url=::url::Url::parse(raw).context("Invalid CLASSIFY_COWORK_URL")?;
            if url.scheme()!="https" && !(url.scheme()=="http" && matches!(url.host_str(),Some("127.0.0.1"|"localhost"))) {
                bail!("CLASSIFY_COWORK_URL must be HTTPS or localhost HTTP");
            }
            if url.username()!="" || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
                bail!("CLASSIFY_COWORK_URL must not embed secrets or query parameters");
            }
        }
        let classify_vibe_key=vibe_ocr_key.clone();
        Ok(Self {
            database_url,
            listen:var("LISTEN","127.0.0.1:8090"),
            synology_url,
            synology_root:root,
            synology_project_name:project_name,
            synology_default_work:work_name,
            synology_username,
            synology_password,
            synology_verify_tls:boolean("SYNOLOGY_VERIFY_TLS",true),
            synology_ca_cert:env::var("SYNOLOGY_CA_CERT").ok().filter(|s|!s.is_empty()).map(PathBuf::from),
            data_root,
            temp_root,
            app_user:var("APP_USER","admin"),
            app_password:password,
            max_file_mb:var("MAX_FILE_MB","100").parse::<u64>()?.max(1),
            ocr_max_pages:var("OCR_MAX_PAGES","500").parse::<usize>()?.clamp(1,5000),
            ocr_mode,
            local_ocr_model_dir,
            ocr_page_workers:var("OCR_PAGE_WORKERS","6").parse::<usize>()?.clamp(1,16),
            vibe_ocr_enabled,
            vibe_ocr_url:var("VIBE_OCR_URL","https://vibecode.bitrix24.tech/v1/ai/chat/completions"),
            vibe_ocr_key,
            vibe_ocr_model:var("VIBE_OCR_MODEL",""),
            vibe_ocr_max_tokens:var("VIBE_OCR_MAX_TOKENS","0").parse::<u32>()?.min(16000),
            vibe_ocr_timeout:var("VIBE_OCR_TIMEOUT_SECONDS","120").parse::<u64>()?.clamp(10,300),
            ai_mode,
            ai_gateway_url:gateway,
            ai_gateway_token:env::var("AI_GATEWAY_TOKEN").ok().filter(|s|!s.is_empty()),
            ai_gigachat_auth_key:ai_key,
            ai_gigachat_scope:var("GIGACHAT_SCOPE","GIGACHAT_API_B2B"),
            ai_gigachat_model:var("GIGACHAT_MODEL","GigaChat-2-Pro"),
            ai_ca_cert:env::var("AI_CA_CERT").ok().filter(|s|!s.is_empty()).map(PathBuf::from),
            classify_vibe_key,
            classify_vibe_model:var("CLASSIFY_VIBE_MODEL", ""),
            classify_cowork_url,
            classify_cowork_token:env::var("CLASSIFY_COWORK_TOKEN").ok().filter(|s|!s.is_empty()),
            classify_timeout:var("CLASSIFY_TIMEOUT_SECONDS","120").parse::<u64>()?.clamp(10,300),
            classify_max_chars:var("CLASSIFY_MAX_CHARS","8000").parse::<usize>()?.clamp(500,20000),
            ai_max_docs:var("AI_MAX_DOCS","6").parse::<usize>()?.clamp(2,12),
            ai_max_chars_per_doc:var("AI_MAX_CHARS_PER_DOC","4000").parse::<usize>()?.clamp(256,12000),
        })
    }
}

#[cfg(test)]
mod storage_path_tests {
    use super::*;
    #[test]
    fn resolves_project_relative_paths() {
        let base=Path::new("CARGO_PROJECT_ROOT");
        assert_eq!(resolve_path(base,"./storage"),base.join("./storage"));
        assert_eq!(resolve_path(base,"./models/ocr"),base.join("./models/ocr"));
    }
    #[test]
    fn absolute_path_is_unchanged() {
        let base=Path::new("other_root");
        let absolute=env::current_dir().expect("cwd");
        assert_eq!(resolve_path(base,absolute.to_str().unwrap()),absolute);
    }
}
