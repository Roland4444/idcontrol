//! Block 1: traceable project requirements, never inferred as legally mandatory
//! without an engineer decision. A proposal points to one immutable project
//! source revision, OCR page/line and excerpt. Only approved required proposals
//! enter the quantitative check of the CURRENT contractor delivery.
use crate::{access, catalog::{self,Catalog}, web::{self,Shared,WebError}};
use anyhow::Result;
use axum::{extract::{Path,State},response::Redirect,Extension,Form,Json};
use serde::Deserialize;
use sha2::{Digest,Sha256};
use sqlx::{PgPool,Row};
use std::collections::{HashMap,HashSet};

fn norm(value:&str)->String{catalog::normalize(value).replace('–',"-").replace('—',"-").split_whitespace().collect::<Vec<_>>().join(" ")}
fn digest(value:&str)->String{hex::encode(Sha256::digest(value.as_bytes()))}
fn safe(s:&str)->String{html_escape::encode_safe(s).to_string()}

#[derive(Debug,Clone)]
pub struct Proposal {
    pub catalog_name:String,pub catalog_code:String,pub title:String,
    pub evidence_excerpt:String,pub source_page:Option<i32>,pub source_line:i32,
    pub extraction_kind:String,pub confidence:f32,pub fingerprint:String,
}

// Conservative evidence patterns for earthworks from Block 1.  Patterns find
// candidate evidence, NOT a conclusion that an act applies to this contract.
// Additional work types use literal catalogue item names below.
const EARTH_SCOPE:&[(&str,&[&str])]=&[
    ("1.4", &["обратную засыпку выполнять", "устройство котлована производить", "разработку грунта выполнять"]),
    ("1.6", &["под ростверком выполняется подготовка", "подготовку основания выполнять"]),
    ("1.7", &["послойным уплотнением", "протоколы определения плотности уплотненного грунта"]),
];
const EARTH:&[(&str,&[&str])]=&[
    ("1.2", &["акт освидетельствования геодезической разбивочной основы","акты сдачи-приемки геодезической разбивочной основы"]),
    ("1.3", &["акт разбивки осей", "акты разбивки осей"]),
    ("1.4", &["акт приемки и освидетельствования работ по устройству котлованов", "акт приемки и освидетельствования работ по устройству обратной засыпки", "акты освидетельствования скрытых работ", "акт приемки и освидетельствования работы по подготовке основания"]),
    ("1.5", &["исполнительные геодезические схемы", "исполнительная геодезическая схема"]),
    ("1.6", &["акт приёмки основания", "акт приемки основания"]),
    ("1.7", &["протоколы определения плотности", "протоколы испытаний уплотнения грунта", "протоколы/ заключения лабораторных испытаний"]),
];

fn marker(line:&str)->Option<i32>{
    let rest=line.trim().strip_prefix("=== Страница ")?.strip_suffix(" ===")?;
    rest.parse::<i32>().ok().filter(|p|*p>0)
}
fn withdrawn(line:&str)->bool {
    ["исключен", "исключена", "отменен", "аннулирован", "удален", "не предусматривается", "не требуется", "не выполняется"]
      .iter().any(|key|line.contains(key))
}
/// Explicit, page/line-aware candidate extraction; never uses broad keyword
/// presence alone as evidence of a required act. Limit repeats per code/source.
pub fn discover(text:&str,catalog:&Catalog)->Vec<Proposal>{
    let mut result=Vec::new();
    let mut page=None;
    let mut seen:HashSet<(String,String,String)>=HashSet::new();
    let mut per_code:HashMap<(String,String),usize>=HashMap::new();
    let works=&catalog.work_types;
    for (index,raw) in text.lines().enumerate(){
        if let Some(n)=marker(raw){page=Some(n);continue;}
        let line=norm(raw);
        if line.chars().count()<22 || line.chars().count()>1400 {continue;}
        // Superseded solutions remain visible, but they cannot become a
        // positive requirement by accidental keyword matching.
        for (code,label,phrase) in [
          ("EX-SHEETPILE","Шпунтовое ограждение исключено","шпунтового ограждения"),
          ("EX-DEWATER","Водопонижение исключено из актуального решения","строительного водопонижения"),
        ] {
            if withdrawn(&line) && line.contains(phrase) {
                let excerpt=raw.trim().chars().take(550).collect::<String>();
                let fingerprint=digest(&format!("Земляные работы\n{code}\n{}\n{}",page.unwrap_or(0),norm(&excerpt)));
                let key=("Земляные работы".to_owned(),code.to_owned());
                if *per_code.get(&key).unwrap_or(&0)<4 && seen.insert((key.0.clone(),key.1.clone(),fingerprint.clone())) {
                    *per_code.entry(key).or_default()+=1;
                    result.push(Proposal{catalog_name:"Земляные работы".into(),catalog_code:code.into(),title:label.into(),
                        evidence_excerpt:excerpt,source_page:page,source_line:(index+1) as i32,
                        extraction_kind:"withdrawn".into(),confidence:0.8,fingerprint});
                }
            }
        }
        // Explicit formatting and phrase are required for an audit trail.
        for work in works {
            for item in &work.items {
                let title=norm(&item.title);
                let (explicit,condition)=if work.name=="Земляные работы" {
                    (EARTH.iter().find(|(code,_)|*code==item.code).map(|(_,phrases)|
                        phrases.iter().any(|p|line.contains(&norm(p)))).unwrap_or(false),
                     EARTH_SCOPE.iter().find(|(code,_)|*code==item.code).map(|(_,phrases)|
                        phrases.iter().any(|p|line.contains(&norm(p)))).unwrap_or(false))
                }else{(title.chars().count()>=25 && line.contains(&title),false)};
                if !explicit && !condition {continue;}
                let key=(work.name.clone(),item.code.clone());
                if *per_code.get(&key).unwrap_or(&0)>=4 {continue;}
                let kind=if withdrawn(&line){"withdrawn"}else if explicit {"document"}else{"project_condition"};
                let excerpt=raw.trim().chars().take(550).collect::<String>();
                let fingerprint=digest(&format!("{}\n{}\n{}\n{}",work.name,item.code,page.unwrap_or(0),norm(&excerpt)));
                if !seen.insert((work.name.clone(),item.code.clone(),fingerprint.clone())){continue;}
                *per_code.entry(key).or_default()+=1;
                result.push(Proposal{catalog_name:work.name.clone(),catalog_code:item.code.clone(),title:item.title.clone(),
                    evidence_excerpt:excerpt,source_page:page,source_line:(index+1) as i32,
                    extraction_kind:kind.into(),confidence:if kind=="document" {0.9}else{0.6},fingerprint});
            }
        }
    }
    result
}

/// Extract candidates for one immutable current source, including sources
/// uploaded BEFORE this release. The query is idempotent; do not purge decisions.
pub async fn extract_source(db:&PgPool,catalog:&Catalog,source_id:i64)->Result<usize>{
    let src=sqlx::query("SELECT project_id,recognized_text,status,is_current FROM project_sources WHERE id=$1")
        .bind(source_id).fetch_one(db).await?;
    if src.try_get::<String,_>("status")?!="ready" || !src.try_get::<bool,_>("is_current")? {return Ok(0);}
    let project_id:i64=src.try_get("project_id")?;
    let text:String=src.try_get("recognized_text")?;
    let catalog=catalog.clone();
    // OCR of a 100+ page PDF is large; proposal extraction must not block an
    // Axum/Tokio request worker.
    let proposals=tokio::task::spawn_blocking(move||discover(&text,&catalog)).await?;
    let mut inserted=0usize;
    for p in proposals {
        let written=sqlx::query("INSERT INTO project_requirement_candidates(project_id,source_id,catalog_name,catalog_code,title,evidence_excerpt,source_page,source_line,extraction_kind,confidence,fingerprint) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT DO NOTHING")
          .bind(project_id).bind(source_id).bind(&p.catalog_name).bind(&p.catalog_code).bind(&p.title)
          .bind(&p.evidence_excerpt).bind(p.source_page).bind(p.source_line).bind(&p.extraction_kind).bind(p.confidence).bind(&p.fingerprint)
          .execute(db).await?;
        inserted+=written.rows_affected() as usize;
    }
    // New evidence must explicitly be reviewed: never silently carry over an
    // earlier approval to a new immutable source revision.
    if inserted>0 {
      sqlx::query("UPDATE packages SET project_requirements_approved_at=NULL,project_requirements_approved_by=NULL,block1_approved_at=NULL,block1_approved_by=NULL WHERE project_id=$1 AND catalog_name IN (SELECT DISTINCT catalog_name FROM project_requirement_candidates WHERE source_id=$2)")
        .bind(project_id).bind(source_id).execute(db).await?;
    }
    Ok(inserted)
}

pub async fn refresh_project(db:&PgPool,catalog:&Catalog,project_id:i64)->Result<usize>{
    let ids:Vec<i64>=sqlx::query_scalar("SELECT id FROM project_sources WHERE project_id=$1 AND status='ready' AND is_current=true ORDER BY id")
      .bind(project_id).fetch_all(db).await?;
    let mut count=0;
    for id in ids {count+=extract_source(db,catalog,id).await?;}
    Ok(count)
}

// Source identity is revision-specific. If any PDF is replaced, the effective
// source signature changes and a prior signoff cannot be used for a new check.
pub async fn source_signature(db:&PgPool,project_id:i64)->Result<String>{
    let rows=sqlx::query("SELECT id,sha256 FROM project_sources WHERE project_id=$1 AND status='ready' AND is_current=true ORDER BY id")
       .bind(project_id).fetch_all(db).await?;
    let payload=rows.iter().map(|r|format!("{}:{}",r.get::<i64,_>("id"),r.get::<String,_>("sha256"))).collect::<Vec<_>>().join(";");
    Ok(digest(&payload))
}

#[derive(Debug,Clone)]
pub struct Approved {pub code:String,pub count:i32,pub candidate_id:i64,pub title:String,pub basis:String,pub page:Option<i32>,pub source:String}
#[derive(Default)]pub struct ChecklistState {pub approved:Vec<Approved>,pub pending:i64,pub signed:bool,pub candidates:i64}

pub async fn checklist(db:&PgPool,package_id:i64)->Result<ChecklistState>{
    let pkg=sqlx::query("SELECT project_id,catalog_name,project_requirements_approved_at FROM packages WHERE id=$1")
        .bind(package_id).fetch_one(db).await?;
    let project_id:i64=pkg.try_get("project_id")?;
    let work:Option<String>=pkg.try_get("catalog_name")?;
    let rows=sqlx::query("SELECT c.id,c.catalog_code,c.title,c.evidence_excerpt,c.source_page,c.extraction_kind,s.filename,s.revision, d.disposition,d.min_count FROM project_requirement_candidates c JOIN project_sources s ON s.id=c.source_id LEFT JOIN package_project_requirement_decisions d ON d.package_id=$1 AND d.candidate_id=c.id WHERE c.project_id=$2 AND c.catalog_name=$3 AND s.is_current AND s.status='ready' ORDER BY c.id")
        .bind(package_id).bind(project_id).bind(work.unwrap_or_default()).fetch_all(db).await?;
    let approved_at:Option<chrono::DateTime<chrono::Utc>>=pkg.try_get("project_requirements_approved_at")?;
    let sign:Option<(String,)> =sqlx::query_as("SELECT source_signature FROM package_project_requirement_approvals WHERE package_id=$1 ORDER BY id DESC LIMIT 1")
        .bind(package_id).fetch_optional(db).await?;
    let signature=source_signature(db,project_id).await?;
    let mut state=ChecklistState{signed:approved_at.is_some() && sign.as_ref().map(|x|x.0.as_str())==Some(signature.as_str()),..Default::default()};
    state.candidates=rows.len() as i64;
    for r in rows {
        let disposition:Option<String>=r.try_get("disposition")?;
        let kind:String=r.try_get("extraction_kind")?;
        // Source withdrawal is displayed for engineer review, never mandatory.
        if kind=="withdrawn" {
            // A withdrawal must be acknowledged by the engineer; it is never
            // counted as a positive requirement.
            if disposition.as_deref()!=Some("not_applicable"){state.pending+=1;}
            continue;
        }
        match disposition.as_deref(){
            Some("required") =>state.approved.push(Approved{code:r.try_get("catalog_code")?,count:r.try_get("min_count")?,candidate_id:r.try_get("id")?,title:r.try_get("title")?,basis:r.try_get("evidence_excerpt")?,page:r.try_get("source_page")?,source:format!("{} (ред. {})",r.get::<String,_>("filename"),r.get::<i32,_>("revision"))}),
            Some("not_applicable")=>{},
            _=>state.pending+=1,
        }
    }
    Ok(state)
}

#[derive(Deserialize)]pub struct GenerateForm{pub csrf:String}
pub async fn generate(State(st):State<Shared>,Extension(actor):Extension<access::Principal>,Path(project_id):Path<i64>,Form(form):Form<GenerateForm>)->Result<Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    if !crate::project_sources::can_upload(&st,&actor,project_id).await {return Err(WebError::forbidden());}
    // Keep one generation per project and return immediately. Large OCR texts
    // must never hold the HTTP request open behind Keenetic reverse proxy.
    let claim=sqlx::query("INSERT INTO project_requirement_generation_jobs(project_id,status,started_at,finished_at,found_count,error_message) VALUES ($1,'running',now(),NULL,0,NULL) ON CONFLICT(project_id) DO UPDATE SET status='running',started_at=now(),finished_at=NULL,found_count=0,error_message=NULL WHERE project_requirement_generation_jobs.status<>'running'")
        .bind(project_id).execute(&st.db).await?;
    if claim.rows_affected()>0 {
        let worker=st.clone();
        tokio::spawn(async move {
            let result=refresh_project(&worker.db,&worker.catalog,project_id).await;
            match result {
                Ok(added)=>{let _=sqlx::query("UPDATE project_requirement_generation_jobs SET status='ready',found_count=$2,finished_at=now() WHERE project_id=$1")
                    .bind(project_id).bind(added as i32).execute(&worker.db).await;},
                Err(err)=>{
                    tracing::error!(project_id,"Project requirements generation failed: {err:#}");
                    let _=sqlx::query("UPDATE project_requirement_generation_jobs SET status='error',error_message=$2,finished_at=now() WHERE project_id=$1")
                        .bind(project_id).bind(format!("{err:#}").chars().take(500).collect::<String>()).execute(&worker.db).await;
                },
            }
        });
    }
    crate::repo::audit(&st.db,&format!("bitrix:{}:{}",actor.member,actor.user_id),"discover_project_requirements",&format!("project={project_id}")).await?;
    Ok(Redirect::to(&format!("/projects/{project_id}#project-requirements")))
}

pub async fn status(State(st):State<Shared>,Extension(actor):Extension<access::Principal>,Path(project_id):Path<i64>)->Result<Json<serde_json::Value>,WebError>{
    if !access::project_visible(&st,&actor,project_id).await {return Err(WebError::forbidden());}
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM project_requirement_candidates c JOIN project_sources s ON s.id=c.source_id WHERE c.project_id=$1 AND s.is_current AND s.status='ready'")
        .bind(project_id).fetch_one(&st.db).await?;
    let record=sqlx::query("SELECT status,found_count,error_message FROM project_requirement_generation_jobs WHERE project_id=$1")
        .bind(project_id).fetch_optional(&st.db).await?;
    let (state,added,error)=match record {
        Some(r)=>(r.try_get::<String,_>("status")?,r.try_get::<i32,_>("found_count")?,r.try_get::<Option<String>,_>("error_message")?),
        None=>("idle".into(),0,None),
    };
    Ok(Json(serde_json::json!({"status":state,"count":count,"added":added,"error":error})))
}

#[derive(Deserialize)]pub struct DecisionForm {pub csrf:String,pub disposition:String,pub min_count:i32}
pub async fn decide(State(st):State<Shared>,Extension(actor):Extension<access::Principal>,Path((package_id,candidate_id)):Path<(i64,i64)>,Form(form):Form<DecisionForm>)->Result<Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    if !matches!(access::package_role(&st,&actor,package_id).await.as_deref(),Some("manager"|"engineer")){return Err(WebError::forbidden());}
    if !["required","not_applicable","review"].contains(&form.disposition.as_str()) || !(1..=999).contains(&form.min_count){return Err(WebError::bad("Недопустимое решение или количество"));}
    let candidate=sqlx::query("SELECT c.extraction_kind FROM project_requirement_candidates c JOIN project_sources s ON s.id=c.source_id JOIN packages p ON p.project_id=c.project_id AND p.catalog_name=c.catalog_name WHERE p.id=$1 AND c.id=$2 AND s.is_current AND s.status='ready'")
        .bind(package_id).bind(candidate_id).fetch_optional(&st.db).await?.ok_or_else(WebError::not_found)?;
    if candidate.try_get::<String,_>("extraction_kind")?=="withdrawn" && form.disposition=="required"{
        return Err(WebError::bad("Отменённое решение нельзя сделать обязательным без отдельного проектного основания"));
    }
    let by=format!("bitrix:{}:{}",actor.member,actor.user_id);
    let mut tx=st.db.begin().await?;
    sqlx::query("INSERT INTO package_project_requirement_decisions(package_id,candidate_id,disposition,min_count,decided_by) VALUES($1,$2,$3,$4,$5) ON CONFLICT(package_id,candidate_id) DO UPDATE SET disposition=EXCLUDED.disposition,min_count=EXCLUDED.min_count,decided_by=EXCLUDED.decided_by,decided_at=now()")
        .bind(package_id).bind(candidate_id).bind(&form.disposition).bind(form.min_count).bind(&by).execute(&mut *tx).await?;
    sqlx::query("UPDATE packages SET project_requirements_approved_at=NULL,project_requirements_approved_by=NULL,block1_approved_at=NULL,block1_approved_by=NULL WHERE id=$1")
        .bind(package_id).execute(&mut *tx).await?;
    tx.commit().await?;
    crate::repo::audit(&st.db,&by,"project_requirement_decision",&format!("package={package_id} candidate={candidate_id} {}",form.disposition)).await?;
    Ok(Redirect::to(&format!("/packages/{package_id}#project-requirements")))
}

#[derive(Deserialize)]pub struct ApproveForm {pub csrf:String}
pub async fn approve(State(st):State<Shared>,Extension(actor):Extension<access::Principal>,Path(package_id):Path<i64>,Form(form):Form<ApproveForm>)->Result<Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    if !matches!(access::package_role(&st,&actor,package_id).await.as_deref(),Some("manager"|"engineer")){return Err(WebError::forbidden());}
    let pkg=crate::repo::package(&st.db,package_id).await?;
    let state=checklist(&st.db,package_id).await?;
    if state.pending>0 || state.candidates==0{return Err(WebError::bad("Для утверждения проектных требований рассмотрите все предложения. Если предложений нет, проверьте распознанный текст и заполните ручной реестр работ."));}
    let sign=source_signature(&st.db,pkg.project_id).await?;
    let snapshot=serde_json::json!({"signature":sign,"required":state.approved.iter().map(|a|serde_json::json!({"candidate_id":a.candidate_id,"code":a.code,"count":a.count,"source":a.source,"page":a.page,"excerpt":a.basis})).collect::<Vec<_>>()});
    let by=format!("bitrix:{}:{}",actor.member,actor.user_id);
    let mut tx=st.db.begin().await?;
    sqlx::query("UPDATE packages SET project_requirements_approved_at=now(),project_requirements_approved_by=$1,block1_approved_at=NULL,block1_approved_by=NULL WHERE id=$2")
        .bind(&by).bind(package_id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO package_project_requirement_approvals(package_id,source_signature,snapshot,approved_by) VALUES($1,$2,$3,$4)")
        .bind(package_id).bind(&sign).bind(snapshot).bind(&by).execute(&mut *tx).await?;
    tx.commit().await?;
    crate::repo::audit(&st.db,&by,"project_requirements_approved",&format!("package={package_id}")).await?;
    Ok(Redirect::to(&format!("/packages/{package_id}#project-requirements")))
}

pub async fn panel(st:&Shared,package_id:i64,actor_role:&str)->Result<String,WebError>{
    let pkg=crate::repo::package(&st.db,package_id).await?;
    let rows=sqlx::query("SELECT c.id,c.catalog_code,c.title,c.evidence_excerpt,c.source_page,c.source_line,c.extraction_kind,s.id AS source_id,s.filename,s.revision,d.disposition,d.min_count FROM project_requirement_candidates c JOIN project_sources s ON s.id=c.source_id LEFT JOIN package_project_requirement_decisions d ON d.package_id=$1 AND d.candidate_id=c.id WHERE c.project_id=$2 AND c.catalog_name=$3 AND s.status='ready' AND s.is_current ORDER BY c.catalog_code,c.id LIMIT 200")
       .bind(package_id).bind(pkg.project_id).bind(pkg.catalog_name.unwrap_or_default()).fetch_all(&st.db).await?;
    let state=checklist(&st.db,package_id).await?;
    let mut out=format!("<article id='project-requirements'><details class=collapsible open><summary>Блок 1 — требования из КР, ПЗУ, ПОС и ППР</summary><div><p>Найдены предложения с дословным источником. Пока инженер не подтвердил применимость, они не становятся обязательными. Предложения с пометкой «отменено» не активируются автоматически.</p><p><b>Предложений:</b> {} · <b>Требуют решения:</b> {} · <b>Утверждение:</b> {}</p>",state.candidates,state.pending,if state.signed{"актуально"}else{"не утверждено / изменены источники"});
    out.push_str(&format!("<p><a class=buttonlink href='/projects/{}#project-requirements'>Исходные документы и формирование предложений</a></p>",pkg.project_id));
    if rows.is_empty(){out.push_str("<p>Предложений пока нет. На странице ЖК нажмите «Сформировать требования из документов». Для непокрытых операций используйте ручной реестр работ.</p>");}
    else {
      out.push_str("<table><thead><tr><th>Пункт</th><th>Дословное основание</th><th>Источник</th><th>Решение инженера</th></tr></thead><tbody>");
      for r in rows {
        let id:i64=r.try_get("id")?;let code:String=r.try_get("catalog_code")?;let title:String=r.try_get("title")?;
        let excerpt:String=r.try_get("evidence_excerpt")?;let file:String=r.try_get("filename")?;
        let revision:i32=r.try_get("revision")?;let page:Option<i32>=r.try_get("source_page")?;
        let source_id:i64=r.try_get("source_id")?;let source_line:i32=r.try_get("source_line")?;
        let kind:String=r.try_get("extraction_kind")?;
        let disposition:Option<String>=r.try_get("disposition")?;
        let count:Option<i32>=r.try_get("min_count")?;
        let source_loc=page.map(|p|format!("стр. {p}")).unwrap_or_else(||format!("строка извлечённого текста {source_line}"));
        let chosen=disposition.as_deref().unwrap_or("review");
        let editor=if matches!(actor_role,"manager"|"engineer") {
          let options=[("review","На рассмотрении"),("required","Обязательно"),("not_applicable","Не применяется")].iter()
             .filter(|(key,_)|kind!="withdrawn" || *key!="required")
             .map(|(key,label)|format!("<option value='{key}' {}>{label}</option>",if chosen==*key{"selected"}else{""})).collect::<String>();
          format!("<form method='post' action='/packages/{package_id}/project-requirements/{id}/decision'>{}<select name=disposition>{options}</select><label>Шт. <input type=number name=min_count min=1 max=999 value={} style='width:65px'></label><button>Сохранить</button></form>",web::hidden(&st.csrf),count.unwrap_or(1))
        }else{safe(chosen)};
        out.push_str(&format!("<tr><td>{} — {}<br><small>{}</small></td><td style='max-width:510px'>{}</td><td>{} · v{revision}<br><small>{source_loc}</small> · <a href='/projects/{}/sources/{source_id}/download'>Оригинал</a></td><td>{editor}</td></tr>",safe(&code),safe(&title),if kind=="withdrawn"{"ОТМЕНЕНО / заменено"}else{"Кандидат"},safe(&excerpt),safe(&file),pkg.project_id));
      }
      out.push_str("</tbody></table>");
    }
    if matches!(actor_role,"manager"|"engineer") && state.candidates>0 {
       out.push_str(&format!("<form method=post action='/packages/{package_id}/project-requirements/approve'>{}<button class=primary>Утвердить проектные требования</button></form>",web::hidden(&st.csrf)));
    }
    out.push_str("<p class=muted>Утверждение действует только для текущих редакций источников. Количество по проекту не подменяет фактический реестр работ, а статусы не означают соответствия актов нормам.</p></div></details></article>");
    Ok(out)
}

pub async fn project_panel(st:&Shared,project_id:i64,actor:&access::Principal)->Result<String,WebError>{
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM project_requirement_candidates c JOIN project_sources s ON s.id=c.source_id WHERE c.project_id=$1 AND s.is_current AND s.status='ready'")
        .bind(project_id).fetch_one(&st.db).await?;
    let allowed=crate::project_sources::can_upload(st,actor,project_id).await;
    let form=if allowed {format!("<form method=post action='/projects/{project_id}/project-requirements/generate'>{}<button class=primary>Сформировать требования из документов</button></form>",web::hidden(&st.csrf))}
        else{"<p>Генерация доступна администратору ЖК и инженеру ПТО.</p>".into()};
    Ok(format!(r#"<section class='white-panel' id='project-requirements' style='padding:20px;margin:16px 0'>
        <h2>Предложения по комплектности ИД из проектных документов</h2>
        <p>Найдено <strong id='req-count'>{count}</strong> подтверждаемых цитат. Это только предложения; окончательную обязательность инженер утверждает в карточке вида работ.</p>
        {form}
        <p><button type=button id='req-refresh'>Обновить количество и статус</button> <small id='req-status' role='status'></small></p>
      </section><script>(function(){{
        const note=document.getElementById('req-status'), count=document.getElementById('req-count');
        if(!note||!count)return;
        async function refresh(){{try{{
          const r=await fetch('/projects/{project_id}/project-requirements/status',{{credentials:'same-origin',cache:'no-store'}});
          if(!r.ok)throw Error('HTTP '+r.status);
          const data=await r.json();count.textContent=String(data.count);
          note.textContent=data.status==='running'?'Анализ проектных документов выполняется…':
            data.status==='ready'?'Анализ завершён. Новых предложений: '+data.added:
            data.status==='error'?'Ошибка анализа: '+data.error:'Анализ ещё не запускался';
        }}catch(e){{note.textContent='Не удалось получить статус: '+String(e)}}}}
        document.getElementById('req-refresh')?.addEventListener('click',refresh);
        refresh();setInterval(()=>{{if(!document.hidden)refresh()}},3000);
      }})();</script>"#))
}

#[cfg(test)]mod tests {
 use super::*;
 #[test] fn extracts_evidence_and_pages(){
    let catalog=Catalog::load().unwrap();
    let text="=== Страница 3 ===\nАкт приемки и освидетельствования работ по устройству котлованов (траншей).\n=== Страница 4 ===\nИсключено устройство шпунтового ограждения.\n=== Страница 5 ===\nАкты освидетельствования скрытых работ.\n";
    let got=discover(text,&catalog);
    assert!(got.iter().any(|x|x.catalog_code=="1.4" && x.source_page==Some(3)));
    assert!(got.iter().any(|x|x.catalog_code=="1.4" && x.source_page==Some(5)));
    assert!(got.iter().any(|x|x.catalog_code=="EX-SHEETPILE" && x.extraction_kind=="withdrawn"));
    let design=discover("=== Страница 24 ===\nОбратную засыпку выполнять непучинистым грунтом с послойным уплотнением (слой 20-30 см).",&catalog);
    assert!(design.iter().any(|x|x.catalog_code=="1.4" && x.extraction_kind=="project_condition" && x.source_page==Some(24)));
    assert!(design.iter().any(|x|x.catalog_code=="1.7" && x.extraction_kind=="project_condition"));
    let pos=discover("=== Страница 3 ===\nУдалена информация об устройстве строительного водопонижения в связи с фактическим отсутствием грунтовых вод",&catalog);
    assert!(pos.iter().any(|x|x.catalog_code=="EX-DEWATER" && x.extraction_kind=="withdrawn"));
 }
 #[test] fn no_fake_requirements_from_generic_terms(){let c=Catalog::load().unwrap();assert!(discover("Проект содержит земляные работы и грунты.",&c).is_empty());}
}
