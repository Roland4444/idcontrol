//! Bitrix24 identities are validated server-side using user.current.
//! BX24 JS obtains short-lived access credentials automatically; no manual tokens.
//! Only an opaque cookie (SHA-256 hash persisted in PostgreSQL) authorizes requests.
use crate::web::Shared;
use axum::{body::Body,extract::State,http::{header,HeaderValue,Method,Request,StatusCode},middleware::Next,response::{IntoResponse,Response},Json};
use base64::Engine;
use rand::RngCore;
use serde::Deserialize;
use serde_json::{json,Value};
use sha2::{Digest,Sha256};
use sqlx::Row;
use std::time::Duration;
use subtle::ConstantTimeEq;

#[derive(Clone,Debug)]
pub struct Principal {pub member:String,pub user_id:i64,pub name:String,pub superuser:bool,pub debug:bool}

pub fn is_super(id:i64)->bool { id==1 || id==336 }
pub fn has_super_role(id:i64,role:&str)->bool { is_super(id) || role=="admin" }
fn digest(token:&str)->String {hex::encode(Sha256::digest(token.as_bytes()))}
fn bad_auth()->Response {(StatusCode::UNAUTHORIZED,"Войдите через приложение в Bitrix24").into_response()}
fn allowed_public(path:&str,method:&Method)->bool {
    (path=="/bitrix" && (method==Method::GET || method==Method::POST)) ||
    (path=="/auth/bitrix" && method==Method::POST) ||
    (path=="/assets/bitrix-client.js" && method==Method::GET) ||
    (path.starts_with("/assets/projects/") && method==Method::GET)
}
fn param_id(path:&str,prefix:&str)->Option<i64>{path.strip_prefix(prefix)?.split('/').next()?.parse().ok()}

// Read request header values *before* awaiting PostgreSQL. Holding `&Request<Body>`
// across an await makes this Axum 0.8 middleware future non-Send.
async fn actor_from_headers(st:&Shared, auth_header:Option<&str>, cookies:Option<&str>)->Option<Principal>{
    if st.debug_login {
        let raw=auth_header?;
        let decoded=base64::engine::general_purpose::STANDARD.decode(raw.strip_prefix("Basic ")?).ok()?;
        let cred=String::from_utf8(decoded).ok()?;
        let (user,pass)=cred.split_once(':')?;
        if bool::from(user.as_bytes().ct_eq(b"admin")) && bool::from(pass.as_bytes().ct_eq(b"super123")) {
            return Some(Principal{member:debug_rbac_namespace(),user_id:1,name:"Администратор (отладка)".into(),superuser:true,debug:true});
        }
        return None;
    }
    let cookie=cookies?;
    let session=cookie.split(';').filter_map(|p|p.trim().split_once('='))
        .find(|(k,_)|*k=="__Host-idcontrol_session").map(|(_,v)|v)?;
    if session.len()!=64 || !session.bytes().all(|b|b.is_ascii_alphanumeric()||b==b'-'||b==b'_'){return None;}
    let row=sqlx::query("SELECT s.member_id,s.user_id,s.display_name,p.global_role FROM portal_sessions s JOIN bitrix_principals p ON p.member_id=s.member_id AND p.user_id=s.user_id WHERE s.token_hash=$1 AND s.expires_at>now() AND (p.active=true OR s.user_id IN (1,336))")
        .bind(digest(session)).fetch_optional(&st.db).await.ok()??;
    let user_id:i64=row.try_get("user_id").ok()?;
    Some(Principal{member:row.try_get("member_id").ok()?,user_id,name:row.try_get("display_name").ok()?,superuser:has_super_role(user_id,&row.try_get::<String,_>("global_role").unwrap_or_default()),debug:false})
}

pub async fn authorize(State(st):State<Shared>,mut request:Request<Body>,next:Next)->Response{
    let path=request.uri().path().to_string();
    if allowed_public(&path,request.method()){return next.run(request).await;}
    // `Request<Body>` is not Sync: do not retain references to it across `.await`.
    let auth_header=request.headers().get(header::AUTHORIZATION)
        .and_then(|v|v.to_str().ok()).map(str::to_owned);
    let cookies=request.headers().get(header::COOKIE)
        .and_then(|v|v.to_str().ok()).map(str::to_owned);
    let Some(actor)=actor_from_headers(&st,auth_header.as_deref(),cookies.as_deref()).await else {
        if st.debug_login {
            return (StatusCode::UNAUTHORIZED,[(header::WWW_AUTHENTICATE,"Basic realm=\"ID-Control debug\"")],"Debug login required").into_response();
        }
        return bad_auth();
    };
    let method=request.method().clone();
    let query=request.uri().query().map(str::to_owned);
    let okay=may_access(&st,&actor,&path,&method,query.as_deref()).await;
    if !okay {return (StatusCode::FORBIDDEN,"Нет доступа к этому разделу или действию").into_response();}
    let actor_label=format!("bitrix:{}:{}{}",actor.member,actor.user_id,if actor.debug{" (debug)"}else{""});
    request.extensions_mut().insert(actor);
    let mut response=next.run(request).await;
    // Preserve a real, verified Bitrix actor even for unchanged legacy form handlers.
    if method==Method::POST && response.status().is_success() {
        let _=crate::repo::audit(&st.db,&actor_label,"http_mutation",&format!("{} => {}",path,response.status())).await;
    }
    response.headers_mut().insert(header::X_CONTENT_TYPE_OPTIONS,HeaderValue::from_static("nosniff"));
    response.headers_mut().insert(header::CACHE_CONTROL,HeaderValue::from_static("no-store"));
    // No X-Frame-Options here: Bitrix24 must embed this application via iframe.
    // Use frame-ancestors CSP for the verified Bitrix24 domain at Nginx.
    response
}

pub async fn is_manager(st:&Shared,a:&Principal,project:i64)->bool{
    if a.superuser{return true;}
    sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM bitrix_project_managers WHERE member_id=$1 AND user_id=$2 AND project_id=$3)")
        .bind(&a.member).bind(a.user_id).bind(project).fetch_one(&st.db).await.unwrap_or(false)
}
pub async fn project_visible(st:&Shared,a:&Principal,project:i64)->bool{
    if is_manager(st,a,project).await {return true;}
    // Explicit project membership permits seeing the project card, but gives NO access
    // to its packages. Legacy per-package grants continue to imply visibility.
    let granted=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM bitrix_project_access WHERE member_id=$1 AND user_id=$2 AND project_id=$3)")
        .bind(&a.member).bind(a.user_id).bind(project).fetch_one(&st.db).await.unwrap_or(false);
    if granted{return true;}
    sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM bitrix_package_acl a JOIN packages p ON p.id=a.package_id WHERE a.member_id=$1 AND a.user_id=$2 AND p.project_id=$3)")
        .bind(&a.member).bind(a.user_id).bind(project).fetch_one(&st.db).await.unwrap_or(false)
}
pub async fn package_role(st:&Shared,a:&Principal,package:i64)->Option<String>{
    if a.superuser{return Some("manager".into());}
    let project=sqlx::query_scalar::<_,i64>("SELECT project_id FROM packages WHERE id=$1")
        .bind(package).fetch_optional(&st.db).await.ok()??;
    if is_manager(st,a,project).await{return Some("manager".into());}
    sqlx::query_scalar::<_,String>("SELECT role FROM bitrix_package_acl WHERE member_id=$1 AND user_id=$2 AND package_id=$3")
        .bind(&a.member).bind(a.user_id).bind(package).fetch_optional(&st.db).await.ok().flatten()
}
async fn may_access(st:&Shared,a:&Principal,path:&str,method:&Method,query:Option<&str>)->bool{
    if path=="/" || path=="/auth/logout" || path=="/api/me" {return true;}
    if path=="/api/employees/known" {
        if a.superuser {return true;}
        return sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM bitrix_project_managers WHERE member_id=$1 AND user_id=$2)")
            .bind(&a.member).bind(a.user_id).fetch_one(&st.db).await.unwrap_or(false);
    }
    if path=="/administration" || path.starts_with("/administration/"){return a.superuser;}
    if path=="/health" || path=="/diagnostics" || path.starts_with("/diagnostics/") || path.starts_with("/norms") || path=="/catalog/errors" || path=="/api/ocr/status" {return a.superuser;}
    if path=="/projects" {return false;} // Projects are fixed; no manual names.
    if let Some(project)=param_id(path,"/projects/") {
        let admin_action=path.ends_with("/works") || path.ends_with("/works/all") || path.ends_with("/assign");
        return if admin_action {is_manager(st,a,project).await} else if method==Method::GET {project_visible(st,a,project).await} else {false};
    }
    if let Some(package)=param_id(path,"/packages/"){
        return match package_role(st,a,package).await {
            Some(role)=> method==Method::GET || role=="manager" || role=="engineer" || (role=="uploader" && path.ends_with("/upload")),
            None=>false,
        };
    }
    let owner=if let Some(id)=param_id(path,"/documents/") {
        sqlx::query_scalar::<_,i64>("SELECT package_id FROM documents WHERE id=$1").bind(id).fetch_optional(&st.db).await.ok().flatten()
    } else if let Some(id)=param_id(path,"/findings/") {
        sqlx::query_scalar::<_,i64>("SELECT package_id FROM findings WHERE id=$1").bind(id).fetch_optional(&st.db).await.ok().flatten()
    } else if let Some(id)=param_id(path,"/ocr/jobs/") {
        sqlx::query_scalar::<_,i64>("SELECT package_id FROM ocr_jobs WHERE id=$1").bind(id).fetch_optional(&st.db).await.ok().flatten()
    } else if let Some(id)=param_id(path,"/ai-suggestions/") {
        sqlx::query_scalar::<_,i64>("SELECT package_id FROM ai_suggestions WHERE id=$1").bind(id).fetch_optional(&st.db).await.ok().flatten()
    } else {None};
    if let Some(package)=owner {
        return match package_role(st,a,package).await {Some(role)=>method==Method::GET || role=="manager" || role=="engineer",None=>false};
    }
    if path=="/api/ocr/jobs" || path=="/ocr/jobs/fragment" {
        let package=query.unwrap_or("").split('&').filter_map(|x|x.split_once('='))
            .find(|(k,_)|*k=="package_id").and_then(|(_,v)|v.parse::<i64>().ok());
        return if let Some(pkg)=package {package_role(st,a,pkg).await.is_some()} else {a.superuser};
    }
    false
}

#[derive(Deserialize)]
pub struct BitrixStart {pub access_token:String,pub domain:String}

// The portal is pinned server-side by BITRIX_DOMAIN; the browser cannot select
// an RBAC namespace by sending an arbitrary member_id. For installations that
// have already assigned roles in v0.9.1, BITRIX_MEMBER_ID remains OPTIONAL:
// when set by the operator, it preserves the existing PostgreSQL ACL namespace.
// New installations derive a stable namespace from the trusted configured domain.
fn debug_rbac_namespace()->String {
    // Debug administration works in the same portal ACL namespace as Bitrix SSO.
    let domain=std::env::var("BITRIX_DOMAIN").unwrap_or_default()
        .trim().trim_start_matches("https://").trim_end_matches('/').to_lowercase();
    if domain.is_empty() && std::env::var("BITRIX_MEMBER_ID").is_err() {"debug".to_owned()}
    else {rbac_namespace(&domain)}
}

fn rbac_namespace(configured_domain:&str)->String{
    std::env::var("BITRIX_MEMBER_ID")
        .ok()
        .map(|v|v.trim().to_owned())
        .filter(|v|!v.is_empty() && !v.starts_with("REPLACE") && !v.starts_with("CHANGE"))
        .unwrap_or_else(||format!("portal:{configured_domain}"))
}

pub async fn login(State(st):State<Shared>,Json(body):Json<BitrixStart>)->Response{
    if st.debug_login {return (StatusCode::FORBIDDEN,"Bitrix24 login disabled in debug mode").into_response();}
    let configured_domain=std::env::var("BITRIX_DOMAIN").unwrap_or_default().trim().trim_start_matches("https://").trim_end_matches('/').to_lowercase();
    if configured_domain.is_empty() || !configured_domain.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'.'||c==b'-') {
        return (StatusCode::SERVICE_UNAVAILABLE,"Set BITRIX_DOMAIN to your Bitrix24 portal hostname").into_response();
    }
    // user_id and member_id are deliberately NOT accepted from the browser.
    // user.current on the configured portal is the source of verified identity.
    if body.domain.trim().to_lowercase()!=configured_domain || body.access_token.len()<12 || body.access_token.len()>4096 {
        return bad_auth();
    }
    let configured_member=rbac_namespace(&configured_domain);
    // Fixed admin-configured origin prevents SSRF. Never construct a URL from caller input.
    let url=format!("https://{configured_domain}/rest/user.current.json");
    let client=match reqwest::Client::builder().timeout(Duration::from_secs(12)).build(){Ok(v)=>v,Err(_)=>return bad_auth()};
    let reply=client.post(&url).form(&[("auth",body.access_token.as_str())]).send().await;
    let response=match reply {Ok(r) if r.status().is_success()=>r, _=>return bad_auth()};
    let value:Value=match response.json().await {Ok(v)=>v,Err(_)=>return bad_auth()};
    if value.get("error").is_some() {return bad_auth();}
    let Some(user)=value.get("result") else{return bad_auth();};
    let Some(id)=user.get("ID").and_then(|v|v.as_i64().or_else(||v.as_str()?.parse().ok())) else{return bad_auth();};
    if id<=0 {return bad_auth();}
    let name=[user.get("NAME"),user.get("LAST_NAME")].iter().filter_map(|v|v.and_then(|v|v.as_str())).collect::<Vec<_>>().join(" ");
    let name=if name.trim().is_empty(){"Сотрудник Bitrix24".to_string()}else{name.chars().take(150).collect()};
    let record=sqlx::query("INSERT INTO bitrix_principals(member_id,user_id,global_role) VALUES ($1,$2,'user') ON CONFLICT(member_id,user_id) DO NOTHING")
        .bind(&configured_member).bind(id).execute(&st.db).await;
    if record.is_err(){return (StatusCode::INTERNAL_SERVER_ERROR,"Failed to register user").into_response();}
    let flags=sqlx::query_as::<_,(bool,String)>("SELECT active,global_role FROM bitrix_principals WHERE member_id=$1 AND user_id=$2")
        .bind(&configured_member).bind(id).fetch_optional(&st.db).await.ok().flatten();
    let (enabled,global_role)=flags.unwrap_or((false,"user".to_owned()));
    // Verified users 1 and 336 are always superadmins, even if the local active flag was disabled.
    if !enabled && !is_super(id) {return (StatusCode::FORBIDDEN,"This Bitrix24 user is disabled").into_response();}
    let mut secret=[0u8;32];rand::thread_rng().fill_bytes(&mut secret);
    let token=hex::encode(secret);
    let stored=sqlx::query("INSERT INTO portal_sessions(token_hash,member_id,user_id,display_name,expires_at) VALUES ($1,$2,$3,$4,now()+interval '6 hours')")
        .bind(digest(&token)).bind(&configured_member).bind(id).bind(&name).execute(&st.db).await;
    if stored.is_err(){return (StatusCode::FORBIDDEN,"Account inactive or database error").into_response();}
    let mut r=Json(json!({"ok":true,"user_id":id,"superadmin":has_super_role(id,&global_role)})).into_response();
    let cookie=format!("__Host-idcontrol_session={token}; Path=/; Max-Age=21600; Secure; HttpOnly; SameSite=None; Partitioned");
    r.headers_mut().insert(header::SET_COOKIE,HeaderValue::from_str(&cookie).expect("safe cookie"));
    r
}

pub async fn logout(State(st):State<Shared>,req:Request<Body>)->Response{
    let cookie=req.headers().get(header::COOKIE).and_then(|v|v.to_str().ok()).unwrap_or("");
    if let Some(token)=cookie.split(';').filter_map(|v|v.trim().split_once('='))
        .find(|(key,_)|*key=="__Host-idcontrol_session").map(|(_,v)|v) {
        let _=sqlx::query("DELETE FROM portal_sessions WHERE token_hash=$1").bind(digest(token)).execute(&st.db).await;
    }
    let mut response=(StatusCode::SEE_OTHER,[(header::LOCATION,"/bitrix")],"").into_response();
    response.headers_mut().insert(header::SET_COOKIE,HeaderValue::from_static("__Host-idcontrol_session=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=None; Partitioned"));
    response
}

pub async fn bitrix_bootstrap()->axum::response::Html<&'static str>{
    axum::response::Html(include_str!("../assets/bitrix-client.html"))
}

// Separate JS file makes the same bootstrap usable for normal launch and installation.
pub async fn bitrix_client_js()->Response{
    ([(header::CONTENT_TYPE,"application/javascript; charset=utf-8"),
      (header::CACHE_CONTROL,"no-store")],include_str!("../assets/bitrix-client.js")).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn built_in_superadministrators(){
        assert!(is_super(1));assert!(is_super(336));
        for id in [0,2,335,337] {assert!(!is_super(id));} // More admins come from PostgreSQL.
    }
    #[test]
    fn assignable_superadministrators(){
        assert!(has_super_role(1,"user"));
        assert!(has_super_role(336,"user"));
        assert!(has_super_role(102,"admin"));
        assert!(!has_super_role(102,"user"));
        assert!(!has_super_role(102,""));
    }
    #[test]
    fn path_parameter_parsing_is_bounded(){
        assert_eq!(param_id("/projects/42/works","/projects/"),Some(42));
        assert_eq!(param_id("/projects/../works","/projects/"),None);
    }
}
