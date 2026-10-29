//! Single-app project and work-type UX. All role decisions are in access.rs.
use crate::{ui_version,access::{self,Principal},models::Package,repo,web::{self,Shared,WebError}};
use axum::{extract::{Path,State},Extension,Form,Json,response::{Html,Redirect,Response,IntoResponse},http::{header,StatusCode}};
use serde::{Deserialize,Serialize};
use sqlx::Row;

pub const PROJECTS:[(&str,&str,&str);6]=[
 ("Федерация","ул. Куйбышева","federation"),
 ("OKLAND","ул. Краснодарская","okland"),
 ("TETRIS","ул. Депутатская/3-я Керченская","tetris"),
 ("КОСМО","ул. Звездная","kosmo"),
 ("PRIME TOWER","ул. Бабефа","prime-tower"),
 ("DUO","ул. Ползунова","duo"),
];
fn h(s:&str)->String{html_escape::encode_safe(s).to_string()}
pub fn project_info(name:&str)->Option<(&'static str,&'static str)>{
    PROJECTS.iter().find(|p|p.0==name).map(|p|(p.1,p.2))
}
fn photo(slug:&str)->String {format!("/assets/projects/{slug}.webp?v={}",ui_version::images_id())}
// Individual, fixed SVG glyphs: no repeated or emoji-dependent icons.
fn work_icon(index:usize)->(String,&'static str){
    const PATHS:[&str;14]=[
      "<path d='M3 19h18M6 16l4-9 4 2-3 7M4 11l4 2M14 7l3-2 4 4-2 3-5-5M12 17l6 2'/>",
      "<rect x='3' y='5' width='18' height='14' rx='1'/><path d='M3 10h18M3 15h18M9 5v5m6 0v5M9 15v4m6-4v4'/>",
      "<circle cx='12' cy='12' r='9'/><circle cx='12' cy='12' r='2'/><path d='M12 10c-4-5-7-4-7-1s3 4 5 4M14 12c4-1 6 2 4 5s-5 1-6-3M11 14c-3 4-1 7 2 6s3-4 1-6'/>",
      "<path d='M12 3C9 7 5 11 5 15a7 7 0 0 0 14 0c0-4-4-8-7-12Z'/><path d='M9 17a3 3 0 0 0 3 2'/>",
      "<circle cx='12' cy='12' r='8'/><circle cx='12' cy='12' r='3'/><path d='M12 4v5m0 6v5m-8-8h5m6 0h5'/>",
      "<path d='M3 18h6V7h6m-3-3h6v6h3v4h-7v-4H9M3 18v3m6-3v3'/><path d='M11 7h7'/>",
      "<path d='M3 6h7v4H7v5h10v4h4M3 10h4m10 9v-4h4M3 6v4M14 4h7v4h-7z'/>",
      "<path d='m13 2-9 12h7l-1 8 10-13h-7l1-7Z'/>",
      "<path d='M12 3v18M7 8h10M5 13h14M3 19h18M7 8 3 19m14-11 4 11M9 13l-4 6m10-6 4 6'/>",
      "<path d='M12 22a8 8 0 0 0 8-8c0-5-4-9-5-11-1 4-3 5-4 6-1-2-1-4-3-6-1 4-4 7-4 11a8 8 0 0 0 8 8Z'/><path d='M12 20a3 3 0 0 0 3-3c0-2-1-3-3-5-1 2-3 3-3 5a3 3 0 0 0 3 3Z'/>",
      "<path d='M12 2 4 5v6c0 5 3 9 8 11 5-2 8-6 8-11V5L12 2Z'/><path d='m8 12 3 3 5-6'/>",
      "<rect x='3' y='10' width='18' height='10' rx='2'/><path d='M8 10V7m8 3V7M7 15h.01M12 15h.01M17 15h.01M5 7a10 10 0 0 1 14 0M8 4a6 6 0 0 1 8 0'/>",
      "<rect x='3' y='4' width='14' height='6' rx='2'/><path d='M17 7h4v5h-9v9M12 21h3M5 7h.01'/>",
      "<path d='M12 3c-2 3-5 4-5 7a5 5 0 0 0 10 0c0-3-3-4-5-7Z'/><path d='M12 15v7m-5 0h10M7 18l5-3 5 3'/>",
    ];
    const TONES:[&str;14]=["blue","coral","purple","cyan","mint","blue","green","orange","violet","red","red","cyan","amber","green"];
    let idx=index.min(13);
    (format!("<svg class='work-svg' xmlns='http://www.w3.org/2000/svg' viewBox='0 0 24 24' fill='none' stroke='currentColor' stroke-width='1.8' stroke-linecap='round' stroke-linejoin='round' aria-hidden='true'>{}</svg>",PATHS[idx]),TONES[idx])
}

fn employee_picker()->&'static str {include_str!("../assets/employee-picker.html")}

pub async fn projects(State(st):State<Shared>,Extension(user):Extension<Principal>)->Result<Html<String>,WebError>{
    let mut cards=String::new();
    let mut available=0usize;
    for (name,address,slug) in PROJECTS {
        let project=sqlx::query("SELECT id FROM projects WHERE name=$1").bind(name).fetch_optional(&st.db).await?;
        let Some(project)=project else {continue};
        let id:i64=project.try_get("id")?;
        if !access::project_visible(&st,&user,id).await {continue;}
        available+=1;
        // Only disclose check runs for accessible work types (superusers/managers see all).
        let manager=access::is_manager(&st,&user,id).await;
        let checked:Option<(i64,Option<chrono::DateTime<chrono::Utc>>)>=sqlx::query_as(
            "SELECT count(DISTINCT r.package_id),max(r.started_at) FROM check_runs r JOIN packages p ON p.id=r.package_id WHERE p.project_id=$1 AND ($2 OR EXISTS (SELECT 1 FROM bitrix_package_acl a WHERE a.package_id=p.id AND a.member_id=$3 AND a.user_id=$4))")
            .bind(id).bind(manager).bind(&user.member).bind(user.user_id).fetch_optional(&st.db).await?;
        let check_note=match checked {
            Some((count,Some(date))) if count>0 => format!("<small class='card-check-summary is-checked'>История проверок: {count} видов работ · {}</small>",date.format("%d.%m.%Y")),
            _=>"<small class='card-check-summary'>Проверок пока нет</small>".to_string(),
        };
        cards.push_str(&format!(r#"<a href="/projects/{id}" class="project-card" aria-label="Открыть жилой комплекс {}">
          <img src="{}" alt="Визуализация ЖК {}" loading="lazy">
          <div class="project-desc"><strong>{}</strong><small>⌖ {}</small>{check_note}<span class="card-arrow" aria-hidden="true">➜</span></div>
        </a>"#,h(name),photo(slug),h(name),h(name),h(address)));
    }
    let empty=if available==0 {"<p class='empty-work'>Вам пока не назначены объекты строительства. Обратитесь к администратору проекта.</p>"}else{""};
    // Normal links must navigate on the FIRST click (and remain keyboard accessible).
    // The old JS cancelled all clicks to show a checkmark and needed a double-click.
    let body=format!(r#"<div class="page-title"><h1>Исполнительная документация</h1><p>Нажмите на карточку жилого комплекса один раз, чтобы открыть виды работ и документы.</p></div>
      <section class="white-panel" id="projects"><div class="panel-heading"><h2><span class="section-icon">▤</span> Жилые комплексы</h2><div class="panel-tools"><input type="search" id="project-search" placeholder="⌕  Поиск по объектам…" aria-label="Поиск по объектам"><span class="pill">{available} объектов</span></div></div>
      <div class="projects-grid">{cards}</div>{empty}<p id="projects-not-found" class="muted" hidden>По запросу жилые комплексы не найдены.</p></section>
      <script>(function(){{
        const cards=Array.from(document.querySelectorAll('.project-card'));
        document.getElementById('project-search')?.addEventListener('input',e=>{{
          const q=e.target.value.trim().toLocaleLowerCase('ru');
          cards.forEach(card=>{{card.hidden=!card.textContent.toLocaleLowerCase('ru').includes(q);}});
          const empty=document.getElementById('projects-not-found');
          if(empty)empty.hidden=cards.some(card=>!card.hidden);
        }});
      }})();</script>"#);
    Ok(web::layout("Объекты строительства",body))
}

pub async fn project(State(st):State<Shared>,Extension(user):Extension<Principal>,Path(id):Path<i64>)->Result<Html<String>,WebError>{
    let row=sqlx::query("SELECT name FROM projects WHERE id=$1").bind(id).fetch_optional(&st.db).await?.ok_or_else(WebError::not_found)?;
    let name:String=row.try_get("name")?;
    let (address,slug)=project_info(&name).ok_or_else(WebError::not_found)?;
    let manager=access::is_manager(&st,&user,id).await;
    let pkgs:Vec<Package>=sqlx::query_as("SELECT id,project_id,name,catalog_name,required_codes_json FROM packages WHERE project_id=$1 ORDER BY name")
        .bind(id).fetch_all(&st.db).await?;
    let mut works=String::new();let mut active=0usize;let mut assigned_table=String::new();
    let assignments:Vec<(i64,i64,String)>=if manager {
        sqlx::query_as("SELECT acl.package_id,acl.user_id,acl.role FROM bitrix_package_acl acl JOIN packages pk ON pk.id=acl.package_id WHERE pk.project_id=$1 AND acl.member_id=$2 ORDER BY acl.user_id")
            .bind(id).bind(&user.member).fetch_all(&st.db).await?
    }else{Vec::new()};
    for (idx,wt) in st.catalog.work_types.iter().enumerate(){
        let existing=pkgs.iter().find(|p|p.name==wt.name);
        let allowed=if let Some(p)=existing {access::package_role(&st,&user,p.id).await.is_some()}else{false};
        if !manager && !allowed {continue;}
        if existing.is_some(){active+=1;}
        let (symbol,tone)=work_icon(idx);
        if let Some(pkg)=existing {
            let files:(i64,)=sqlx::query_as("SELECT count(*) FROM documents WHERE package_id=$1 AND is_present=true")
                .bind(pkg.id).fetch_one(&st.db).await?;
            let latest_check:Option<(i64,chrono::DateTime<chrono::Utc>)>=sqlx::query_as(
                "SELECT id,started_at FROM check_runs WHERE package_id=$1 ORDER BY id DESC LIMIT 1")
                .bind(pkg.id).fetch_optional(&st.db).await?;
            let (check_class,check_status)=if let Some((run,at))=latest_check {
                ("has-check",format!("Проверка #{} · {}",run,at.format("%d.%m.%Y %H:%M")))
            }else if files.0>0 {("needs-check","Проверка ещё не запускалась".to_string())}
            else {("no-files","Документы не загружены".to_string())};
            works.push_str(&format!(r#"<a class="work-card" href="/packages/{}"><span class="work-symbol {}">{}</span><div><strong>{}</strong><small>Комплект ИД · {} файлов</small><small class="work-check {}">{}</small></div><span class="work-arrow">›</span></a>"#,pkg.id,tone,symbol,h(&wt.name),files.0,check_class,h(&check_status)));
            if manager {
                let names=assignments.iter().filter(|(pid,_,_)|*pid==pkg.id)
                    .map(|(_,uid,role)|format!("<span data-employee-id=\"{}\">Сотрудник Bitrix24</span> — {}",uid,h(role)))
                    .collect::<Vec<_>>();
                let members=if names.is_empty(){"Пока никто не назначен".to_owned()}else{names.join("; ")};
                assigned_table.push_str(&format!(r#"<tr><td><strong>{}</strong><small class="assigned-users">{}</small></td><td><form class="assign-form" method="post" action="/projects/{id}/assign">{}<input type="hidden" name="package_id" value="{}"><select class="employee-picker" name="user_id" required aria-label="Выбрать сотрудника"><option value="">Выберите сотрудника</option></select><select name="role"><option value="uploader">Пользователь (просмотр назначенных работ)</option><option value="engineer">Инженер ПТО</option><option value="viewer">Просмотр</option><option value="remove">Снять доступ</option></select><button>Применить</button></form></td></tr>"#,h(&wt.name),h(&members),web::hidden(&st.csrf),pkg.id));
            }
        } else if manager {
            works.push_str(&format!(r#"<form method="post" action="/projects/{id}/works" class="work-card work-add">{}<input type="hidden" name="work_type" value="{}"><span class="work-symbol {}">{}</span><div><strong>{}</strong><small>Ещё не создан · нажмите для добавления</small></div><button title="Создать комплект">＋</button></form>"#,web::hidden(&st.csrf),h(&wt.name),tone,symbol,h(&wt.name)));
        }
    }
    let bulk=if manager {format!(r#"<form method="post" action="/projects/{id}/works/all" class="inline">{}<button class="primary">＋ Создать все 14 видов работ</button></form>"#,web::hidden(&st.csrf))} else {String::new()};
    let assignment=if manager {format!(r#"<details class="admin-management"><summary>Управление доступом сотрудников к видам работ</summary><div class="section-copy">Выберите сотрудника по имени. Права назначаются на сервере и действуют для документов и проверок.</div><table><thead><tr><th>Вид работ</th><th>Назначить сотрудника</th></tr></thead><tbody>{assigned_table}</tbody></table>{}</details>"#,employee_picker())}else{String::new()};
    let sources_panel=crate::project_sources::panel(&st,&user,id).await?;
    let requirements_panel=crate::project_requirements::project_panel(&st,id,&user).await?;
    let empty=if works.is_empty() {"<p class='empty-work'>Доступных видов работ пока нет. Администратор проекта должен создать комплект и назначить вам доступ.</p>"}else{""};
    let body=format!(r#"<div class="crumb"><a href="/">← К выбору объектов</a> <span>›</span> {}</div>
      <section class="project-hero" style="--hero-image:url('{}')"><div class="project-hero-content"><h1>{}</h1><p>⌖ {}, Астрахань</p><div class="hero-stats"><span>▤ Жилой комплекс</span><span>▥ Исполнительная документация</span><span>◫ {active} из 14 разделов</span></div></div></section>
      <div class="workflow-hint"><strong>Как проверить документы?</strong><span>1. Загрузите КР, ПЗУ, ПОС, ППР и РД в библиотеку ЖК ниже → 2. Откройте вид работ и получите ИД от подрядчика по одноразовой ссылке → 3. Утвердите применимые требования → 4. Запустите проверку.</span><small>Проектные документы подключаются ко всем созданным видам работ; исходные ZIP подрядчика остаются отдельными.</small></div>
      {sources_panel}{requirements_panel}<div class="section-heading"><div><h2>Комплекты исполнительной документации</h2><p>Одно нажатие на вид работ открывает загрузку и проверку конкретного комплекта.</p></div><span class="counter">{active} разделов создано</span>{bulk}</div>
      <div class="works-grid">{works}</div>{empty}{assignment}"#,h(&name),photo(slug),h(&name),h(address));
    Ok(web::layout(&name,body))
}

#[derive(Deserialize)]pub struct WorkForm{pub csrf:String,pub work_type:String}
#[derive(Deserialize)]pub struct CsrfForm{pub csrf:String}
#[derive(Deserialize)]pub struct AssignmentForm{pub csrf:String,pub package_id:i64,pub user_id:i64,pub role:String,#[serde(default)] pub bitrix_token:Option<String>}
#[derive(Deserialize)]pub struct ManagerForm{pub csrf:String,pub project_id:i64,pub user_id:i64,pub action:String,#[serde(default)] pub bitrix_token:Option<String>}
#[derive(Deserialize)]pub struct ProjectAccessForm{pub csrf:String,pub project_id:i64,pub user_id:i64,pub action:String,#[serde(default)] pub bitrix_token:Option<String>}
#[derive(Deserialize)]pub struct SuperadminForm{pub csrf:String,pub user_id:i64,pub action:String,#[serde(default)] pub bitrix_token:Option<String>}
#[derive(Deserialize)]pub struct NasMappingForm{pub csrf:String,pub source_name:String,pub project_id:i64,pub default_work:String,pub action:String}

pub async fn create_work(State(st):State<Shared>,Extension(actor):Extension<Principal>,Path(id):Path<i64>,Form(form):Form<WorkForm>)->Result<Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    if !access::is_manager(&st,&actor,id).await{return Err(WebError::forbidden());}
    let wt=st.catalog.work_type(&form.work_type).ok_or_else(||WebError::bad("Выберите вид работ из справочника"))?;
    let project_name=sqlx::query_scalar::<_,String>("SELECT name FROM projects WHERE id=$1")
        .bind(id).fetch_optional(&st.db).await?.ok_or_else(WebError::not_found)?;
    if project_info(&project_name).is_none(){return Err(WebError::not_found());}
    let pkg=repo::get_or_create_package(&st.db,id,&wt.name).await?;
    sqlx::query("UPDATE packages SET catalog_name=$1 WHERE id=$2 AND catalog_name IS NULL").bind(&wt.name).bind(pkg.id).execute(&st.db).await?;
    crate::project_sources::sync_project_to_package(&st.db,id,pkg.id).await?;
    repo::audit(&st.db,&format!("bitrix:{}:{}",actor.member,actor.user_id),"create_work",&format!("project={id} package={}",pkg.id)).await?;
    Ok(Redirect::to(&format!("/projects/{id}")))
}
pub async fn create_all(State(st):State<Shared>,Extension(actor):Extension<Principal>,Path(id):Path<i64>,Form(form):Form<CsrfForm>)->Result<Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    if !access::is_manager(&st,&actor,id).await{return Err(WebError::forbidden());}
    let name=sqlx::query_scalar::<_,String>("SELECT name FROM projects WHERE id=$1").bind(id).fetch_optional(&st.db).await?.ok_or_else(WebError::not_found)?;
    if project_info(&name).is_none(){return Err(WebError::not_found());}
    let mut tx=st.db.begin().await?;
    for wt in &st.catalog.work_types {
        sqlx::query("INSERT INTO packages(project_id,name,catalog_name) VALUES ($1,$2,$2) ON CONFLICT(project_id,name) DO UPDATE SET catalog_name=COALESCE(packages.catalog_name,EXCLUDED.catalog_name)")
            .bind(id).bind(&wt.name).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    let packages:Vec<i64>=sqlx::query_scalar("SELECT id FROM packages WHERE project_id=$1").bind(id).fetch_all(&st.db).await?;
    for package_id in packages { crate::project_sources::sync_project_to_package(&st.db,id,package_id).await?; }
    repo::audit(&st.db,&format!("bitrix:{}:{}",actor.member,actor.user_id),"create_all_works",&format!("project={id}")).await?;
    Ok(Redirect::to(&format!("/projects/{id}")))
}
// A selected ID from the browser is not proof of employment. Verify each new
// permission grant against the configured Bitrix24 tenant (not the browser's
// domain) and require an ACTIVE employee, never an extranet/guest account.
fn eligible_staff_response(value:&serde_json::Value, user_id:i64)->bool {
    value.get("result").and_then(|x|x.as_array()).map(|users|users.iter().any(|u| {
        let id=u.get("ID").and_then(|v|v.as_i64().or_else(||v.as_str()?.parse().ok()));
        let active=u.get("ACTIVE").map(|v|v.as_bool()==Some(true) || v.as_str()==Some("Y") || v.as_str()==Some("true") || v.as_i64()==Some(1)).unwrap_or(false);
        id==Some(user_id) && active && u.get("USER_TYPE").and_then(|v|v.as_str())==Some("employee")
    })).unwrap_or(false)
}
async fn require_active_bitrix_employee(actor:&Principal,user_id:i64,token:Option<&str>)->Result<(),WebError>{
    // Local debug mode retains its established editable-test behavior. Only a
    // real Bitrix24 session can confirm a user's live employment status.
    if actor.debug{return Ok(());}
    let token=token.unwrap_or("").trim();
    if token.len()<12 || token.len()>4096{return Err(WebError::bad("Для назначения сотрудника откройте приложение в Bitrix24 и обновите страницу"));}
    let domain=std::env::var("BITRIX_DOMAIN").unwrap_or_default().trim().trim_start_matches("https://").trim_end_matches('/').to_ascii_lowercase();
    if domain.is_empty() || !domain.bytes().all(|c|c.is_ascii_alphanumeric() || c==b'.' || c==b'-') {
        return Err(WebError::bad("Не настроен домен портала Bitrix24"));
    }
    let url=format!("https://{domain}/rest/user.get.json");
    let id_string=user_id.to_string();
    let client=reqwest::Client::builder().timeout(std::time::Duration::from_secs(12))
        .build().map_err(|_|WebError::bad("Ошибка соединения с Bitrix24"))?;
    let resp=client.post(&url).form(&[("auth",token),("ID",id_string.as_str())]).send().await
        .map_err(|_|WebError::bad("Не удалось проверить сотрудника в Bitrix24"))?;
    if !resp.status().is_success(){return Err(WebError::bad("Bitrix24 не подтвердил сотрудника. Обновите приложение"));}
    let data:serde_json::Value=resp.json().await.map_err(|_|WebError::bad("Неправильный ответ Bitrix24"))?;
    if !eligible_staff_response(&data,user_id){return Err(WebError::bad("Права можно назначить только активному сотруднику Bitrix24 (без гостей)"));}
    Ok(())
}

pub async fn assign_work(State(st):State<Shared>,Extension(actor):Extension<Principal>,Path(id):Path<i64>,Form(form):Form<AssignmentForm>)->Result<Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    if !access::is_manager(&st,&actor,id).await{return Err(WebError::forbidden());}
    if form.user_id<1{return Err(WebError::bad("Выберите сотрудника Bitrix24"));}
    let belongs=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM packages WHERE id=$1 AND project_id=$2)")
        .bind(form.package_id).bind(id).fetch_one(&st.db).await?;
    if !belongs{return Err(WebError::bad("Комплект не относится к этому объекту"));}
    if form.role=="remove"{
        sqlx::query("DELETE FROM bitrix_package_acl WHERE member_id=$1 AND user_id=$2 AND package_id=$3")
          .bind(&actor.member).bind(form.user_id).bind(form.package_id).execute(&st.db).await?;
    }else{
        if !["uploader","viewer","engineer"].contains(&form.role.as_str()) {return Err(WebError::bad("Недопустимая роль"));}
        require_active_bitrix_employee(&actor,form.user_id,form.bitrix_token.as_deref()).await?;
        sqlx::query("INSERT INTO bitrix_principals(member_id,user_id) VALUES ($1,$2) ON CONFLICT(member_id,user_id) DO NOTHING")
            .bind(&actor.member).bind(form.user_id).execute(&st.db).await?;
        sqlx::query("INSERT INTO bitrix_package_acl(member_id,user_id,package_id,role) VALUES ($1,$2,$3,$4) ON CONFLICT(member_id,user_id,package_id) DO UPDATE SET role=EXCLUDED.role")
            .bind(&actor.member).bind(form.user_id).bind(form.package_id).bind(&form.role).execute(&st.db).await?;
    }
    repo::audit(&st.db,&format!("bitrix:{}:{}",actor.member,actor.user_id),"assign_work",&format!("project={id} package={} user={} role={}",form.package_id,form.user_id,form.role)).await?;
    Ok(Redirect::to(&format!("/projects/{id}")))
}

// Local debug directory: users previously authenticated in this portal.
// Production dropdowns read live ACTIVE employees directly from Bitrix24.
#[derive(Serialize)]
pub struct KnownEmployee {user_id:i64,name:String}

pub async fn known_employees(State(st):State<Shared>,Extension(actor):Extension<Principal>)->Result<Json<Vec<KnownEmployee>>,WebError>{
    let rows:Vec<(i64,String)>=sqlx::query_as(r#"
        SELECT p.user_id,COALESCE(NULLIF((SELECT s.display_name FROM portal_sessions s
          WHERE s.member_id=p.member_id AND s.user_id=p.user_id
          ORDER BY s.created_at DESC LIMIT 1),''),'Сотрудник Bitrix24')
        FROM bitrix_principals p WHERE p.member_id=$1 AND p.active=true ORDER BY p.user_id"#)
        .bind(&actor.member).fetch_all(&st.db).await?;
    Ok(Json(rows.into_iter().map(|(user_id,name)|KnownEmployee{user_id,name}).collect()))
}

pub async fn administration(State(st):State<Shared>,Extension(actor):Extension<Principal>)->Result<Html<String>,WebError>{
    if !actor.superuser{return Err(WebError::forbidden());}
    // The built-in superadministrators are immutable and intentionally not
    // exposed as editable rows; their IDs remain only in access::is_super.
    let super_rows:Vec<(i64,String)>=sqlx::query_as(r#"
        SELECT p.user_id,COALESCE(NULLIF((SELECT s.display_name
          FROM portal_sessions s WHERE s.member_id=p.member_id AND s.user_id=p.user_id
          ORDER BY s.created_at DESC LIMIT 1),''),'Сотрудник Bitrix24')
        FROM bitrix_principals p
        WHERE p.member_id=$1 AND p.global_role='admin'
        ORDER BY p.user_id"#).bind(&actor.member).fetch_all(&st.db).await?;
    let mut super_table=String::new();
    for (uid,name) in &super_rows {
        if access::is_super(*uid) {continue;}
        let action=format!(r#"<form method="post" action="/administration/superadmins" class="inline">{}<input type="hidden" name="user_id" value="{}"><input type="hidden" name="action" value="revoke"><button class="btn-subtle" type="submit">Отозвать права</button></form>"#,web::hidden(&st.csrf),uid);
        super_table.push_str(&format!("<tr><td><strong data-employee-id=\"{uid}\">{}</strong></td><td>Дополнительно назначен</td><td>{action}</td></tr>",h(name)));
    }
    if super_table.is_empty(){super_table.push_str("<tr><td colspan='3'>Дополнительные суперадминистраторы пока не назначены</td></tr>");}
    let managers:Vec<(i64,String,String)>=sqlx::query_as(r#"
        SELECT m.user_id,p.name,COALESCE(NULLIF((SELECT s.display_name FROM portal_sessions s
        WHERE s.member_id=m.member_id AND s.user_id=m.user_id ORDER BY s.created_at DESC LIMIT 1),''),'Сотрудник Bitrix24')
        FROM bitrix_project_managers m JOIN projects p ON p.id=m.project_id
        WHERE m.member_id=$1 ORDER BY p.name,m.user_id"#).bind(&actor.member).fetch_all(&st.db).await?;
    let mut trs=String::new();for (user_id,name,person) in managers {
        trs.push_str(&format!("<tr><td>{}</td><td data-employee-id=\"{user_id}\">{}</td></tr>",h(&name),h(&person)));
    }
    if trs.is_empty(){trs.push_str("<tr><td colspan='2'>Администраторы проектов пока не назначены</td></tr>");}
    let project_members:Vec<(i64,String,String)>=sqlx::query_as(r#"
        SELECT a.user_id,p.name,COALESCE(NULLIF((SELECT s.display_name FROM portal_sessions s
        WHERE s.member_id=a.member_id AND s.user_id=a.user_id ORDER BY s.created_at DESC LIMIT 1),''),'Сотрудник Bitrix24')
        FROM bitrix_project_access a JOIN projects p ON p.id=a.project_id
        WHERE a.member_id=$1 ORDER BY p.name,a.user_id"#).bind(&actor.member).fetch_all(&st.db).await?;
    let mut member_rows=String::new();
    for (uid,project_name,person) in project_members {
        member_rows.push_str(&format!("<tr><td>{}</td><td data-employee-id=\"{uid}\">{}</td><td>Доступ к проекту (без доступа к комплектам)</td></tr>",h(&project_name),h(&person)));
    }
    if member_rows.is_empty(){member_rows.push_str("<tr><td colspan='3'>Пользователи проектов пока не назначены</td></tr>");}
    let mut options=String::new();for (name,_,_) in PROJECTS {
        let row=sqlx::query("SELECT id FROM projects WHERE name=$1").bind(name).fetch_one(&st.db).await?;
        let id:i64=row.try_get("id")?;
        options.push_str(&format!("<option value='{id}'>{}</option>",h(name)));
    }
    let links:Vec<(String,String,String)>=sqlx::query_as("SELECT m.source_name,p.name,m.default_work FROM synology_project_mappings m JOIN projects p ON p.id=m.project_id ORDER BY m.source_name")
        .fetch_all(&st.db).await?;
    let mut mapping_rows=String::new();
    for (source,target,work) in links {
        mapping_rows.push_str(&format!("<tr><td>{}</td><td>{}</td><td>{}</td></tr>",h(&source),h(&target),if work.is_empty(){"Из папки на NAS".to_owned()}else{h(&work)}));
    }
    let mut work_options=String::from("<option value=''>Из структуры NAS (по умолчанию — из .env)</option>");
    for wt in &st.catalog.work_types{work_options.push_str(&format!("<option value='{}'>{}</option>",h(&wt.name),h(&wt.name)));}
    let active_synology_root=crate::contractor::selected_root(&st).await?;
    let body=format!(r#"<div class="page-title"><h1>Администрирование</h1><p>Сотрудники, роли, проекты и подключённые папки Synology.</p></div>
    <section class="white-panel" id="superadmins"><h2>Суперадминистраторы</h2><p>Полный доступ ко всем проектам, проверкам и настройкам. Для назначения доступны только действующие сотрудники Bitrix24 — без гостей и внешних пользователей. Поиск по имени или фамилии.</p>
    <form class="admin-form" method="post" action="/administration/superadmins">{}<label>Сотрудник<select name="user_id" class="employee-picker" required><option value="">Выберите сотрудника</option></select></label><button class="primary" name="action" value="grant">Назначить суперадминистратором</button></form>
    <table><thead><tr><th>Сотрудник</th><th>Тип доступа</th><th>Действия</th></tr></thead><tbody>{super_table}</tbody></table>
    <p class="section-copy">Системные суперадминистраторы получают доступ автоматически и не отображаются в списке назначений. Дополнительно назначенные права можно отозвать.</p></section>
    <section class="white-panel"><h2>Администраторы проектов</h2><p>Администратор создаёт виды работ и распределяет доступ сотрудников только внутри своего ЖК.</p>
    <form class="admin-form" method="post" action="/administration/managers">{}<label>Проект<select name="project_id">{options}</select></label><label>Сотрудник<select name="user_id" class="employee-picker" required><option value="">Выберите сотрудника</option></select></label><label>Действие<select name="action"><option value="grant">Назначить</option><option value="revoke">Снять назначение</option></select></label><button>Сохранить</button></form>
    <table><thead><tr><th>Жилой комплекс</th><th>Администратор проекта</th></tr></thead><tbody>{trs}</tbody></table></section>
    <section class="white-panel"><h2>Доступ обычных сотрудников к проектам</h2>
    <p>Доступ к ЖК позволяет увидеть проект, но не открывает документы. На конкретные виды работ назначает администратор проекта.</p>
    <form class="admin-form" method="post" action="/administration/projects/access">{}<label>Жилой комплекс<select name="project_id">{options}</select></label><label>Сотрудник<select name="user_id" class="employee-picker" required><option value="">Выберите сотрудника</option></select></label><label>Действие<select name="action"><option value="grant">Предоставить доступ</option><option value="revoke">Убрать доступ к проекту</option></select></label><button>Сохранить</button></form>
    <table><thead><tr><th>Проект</th><th>Сотрудник</th><th>Права</th></tr></thead><tbody>{member_rows}</tbody></table>
    <p class="section-copy">При отзыве доступа к проекту удаляются назначения на виды работ этого проекта. Назначение администратором проекта снимается отдельно.</p></section>
    <section class="white-panel"><h2>Корневая папка проектов на Synology</h2><p>Выберите каталог внутри /ResData. Для каждого ЖК и вида работ он будет использоваться автоматически при передаче архивов подрядчиками.</p><a class="buttonlink" href="/administration/storage">Выбрать папку в Synology →</a></section>
    <section class="white-panel" id="synology-mapping"><h2>Synology → жилые комплексы</h2>
    <p>Устаревшее сопоставление для исторических импортов с NAS. При загрузке ZIP через ID-Control привязка к ЖК и виду работ определяется одноразовой ссылкой; эта таблица не участвует в обработке новых поставок.</p>
    <p>Текущий корень: <code>{}</code>; имя по умолчанию: <code>{}</code>. Периодический обход Synology отключён; новые архивы связываются с проектом через ссылку загрузки ID-Control. Старые файлы в базе сохраняются.</p>
    <form method="post" action="/administration/synology-map">{}<label>Исходная папка NAS<input name="source_name" required maxlength="200" value="{}"></label><label>Сопоставить с ЖК<select name="project_id">{options}</select></label><label>Вид работ для плоского корня<select name="default_work">{work_options}</select></label><label>Действие<select name="action"><option value="save">Сохранить сопоставление</option><option value="remove">Удалить сопоставление</option></select></label><button>Применить</button></form>
    <table><thead><tr><th>Папка NAS</th><th>Жилой комплекс</th><th>Вид работ для плоского корня</th></tr></thead><tbody>{mapping_rows}</tbody></table></section>
    <section class="white-panel"><h2>Роли и ограничения</h2><p><strong>Суперадминистратор</strong> управляет всей системой и назначает роли. <strong>Администратор проекта</strong> отвечает за виды работ своего ЖК. <strong>Пользователь</strong> видит только назначенные проекты и комплекты.</p><p>Права проверяются сервером при каждом обращении, включая скачивание документов и запуск OCR.</p></section>
    {}"#,web::hidden(&st.csrf),web::hidden(&st.csrf),web::hidden(&st.csrf),h(&active_synology_root),h(&st.cfg.synology_project_name),web::hidden(&st.csrf),h(&st.cfg.synology_project_name),employee_picker());
    Ok(web::layout("Администрирование",body))
}

pub async fn assign_superadmin(State(st):State<Shared>,Extension(actor):Extension<Principal>,Form(form):Form<SuperadminForm>)->Result<Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    if !actor.superuser{return Err(WebError::forbidden());}
    if form.user_id<1{return Err(WebError::bad("Выберите сотрудника Bitrix24"));}
    match form.action.as_str() {
        "grant"=>{
            require_active_bitrix_employee(&actor,form.user_id,form.bitrix_token.as_deref()).await?;
            sqlx::query("INSERT INTO bitrix_principals(member_id,user_id,active,global_role) VALUES ($1,$2,true,'admin') ON CONFLICT(member_id,user_id) DO UPDATE SET global_role='admin',active=true")
                .bind(&actor.member).bind(form.user_id).execute(&st.db).await?;
        },
        "revoke"=>{
            if access::is_super(form.user_id){return Err(WebError::bad("Нельзя отозвать базовые права суперадминистратора"));}
            sqlx::query("UPDATE bitrix_principals SET global_role='user' WHERE member_id=$1 AND user_id=$2")
                .bind(&actor.member).bind(form.user_id).execute(&st.db).await?;
        },
        _=>return Err(WebError::bad("Недопустимая операция")),
    }
    repo::audit(&st.db,&format!("bitrix:{}:{}",actor.member,actor.user_id),"superadmin_role_changed",
        &format!("target={} action={}",form.user_id,form.action)).await?;
    Ok(Redirect::to("/administration#superadmins"))
}

pub async fn assign_project_access(State(st):State<Shared>,Extension(actor):Extension<Principal>,Form(form):Form<ProjectAccessForm>)->Result<Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    if !actor.superuser{return Err(WebError::forbidden());}
    if form.user_id<1{return Err(WebError::bad("Выберите сотрудника Bitrix24"));}
    let project_name=sqlx::query_scalar::<_,String>("SELECT name FROM projects WHERE id=$1")
        .bind(form.project_id).fetch_optional(&st.db).await?.ok_or_else(WebError::not_found)?;
    if project_info(&project_name).is_none(){return Err(WebError::not_found());}
    match form.action.as_str(){
        "grant"=>{
            require_active_bitrix_employee(&actor,form.user_id,form.bitrix_token.as_deref()).await?;
            sqlx::query("INSERT INTO bitrix_principals(member_id,user_id) VALUES ($1,$2) ON CONFLICT(member_id,user_id) DO NOTHING")
                .bind(&actor.member).bind(form.user_id).execute(&st.db).await?;
            sqlx::query("INSERT INTO bitrix_project_access(member_id,user_id,project_id,granted_by) VALUES ($1,$2,$3,$4) ON CONFLICT(member_id,user_id,project_id) DO UPDATE SET granted_by=EXCLUDED.granted_by,granted_at=now()")
                .bind(&actor.member).bind(form.user_id).bind(form.project_id).bind(actor.user_id).execute(&st.db).await?;
        },
        "revoke"=>{
            // A project-level revoke also removes package grants; otherwise a former
            // member could still see this project through their old work ACLs.
            let mut tx=st.db.begin().await?;
            sqlx::query("DELETE FROM bitrix_project_access WHERE member_id=$1 AND user_id=$2 AND project_id=$3")
                .bind(&actor.member).bind(form.user_id).bind(form.project_id).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM bitrix_package_acl a USING packages p WHERE a.package_id=p.id AND a.member_id=$1 AND a.user_id=$2 AND p.project_id=$3")
                .bind(&actor.member).bind(form.user_id).bind(form.project_id).execute(&mut *tx).await?;
            tx.commit().await?;
        },
        _=>return Err(WebError::bad("Недопустимая операция")),
    }
    repo::audit(&st.db,&format!("bitrix:{}:{}",actor.member,actor.user_id),"project_user_access",
        &format!("project={} user={} action={}",form.project_id,form.user_id,form.action)).await?;
    Ok(Redirect::to("/administration"))
}

pub async fn assign_manager(State(st):State<Shared>,Extension(actor):Extension<Principal>,Form(form):Form<ManagerForm>)->Result<Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    if !actor.superuser || form.user_id<1{return Err(WebError::forbidden());}
    let project=sqlx::query_scalar::<_,String>("SELECT name FROM projects WHERE id=$1").bind(form.project_id).fetch_optional(&st.db).await?.ok_or_else(WebError::not_found)?;
    if project_info(&project).is_none(){return Err(WebError::not_found());}
    if form.action=="revoke"{
        sqlx::query("DELETE FROM bitrix_project_managers WHERE member_id=$1 AND user_id=$2 AND project_id=$3")
            .bind(&actor.member).bind(form.user_id).bind(form.project_id).execute(&st.db).await?;
    }else if form.action=="grant" {
        require_active_bitrix_employee(&actor,form.user_id,form.bitrix_token.as_deref()).await?;
        sqlx::query("INSERT INTO bitrix_principals(member_id,user_id) VALUES ($1,$2) ON CONFLICT(member_id,user_id) DO NOTHING")
            .bind(&actor.member).bind(form.user_id).execute(&st.db).await?;
        sqlx::query("INSERT INTO bitrix_project_managers(member_id,user_id,project_id,granted_by) VALUES ($1,$2,$3,$4) ON CONFLICT(member_id,user_id,project_id) DO UPDATE SET granted_by=EXCLUDED.granted_by,granted_at=now()")
            .bind(&actor.member).bind(form.user_id).bind(form.project_id).bind(actor.user_id).execute(&st.db).await?;
    }else{return Err(WebError::bad("Недопустимая операция"));}
    repo::audit(&st.db,&format!("bitrix:{}:{}",actor.member,actor.user_id),"manager_assignment",&format!("project={} user={} {}",form.project_id,form.user_id,form.action)).await?;
    Ok(Redirect::to("/administration"))
}


pub async fn save_synology_mapping(State(st):State<Shared>,Extension(actor):Extension<Principal>,Form(form):Form<NasMappingForm>)->Result<Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    if !actor.superuser{return Err(WebError::forbidden());}
    let source=form.source_name.trim();
    repo::validate_segment(source).map_err(|_|WebError::bad("Неверное имя папки Synology"))?;
    if source.len()>200{return Err(WebError::bad("Слишком длинное имя папки"));}
    let by=format!("bitrix:{}:{}",actor.member,actor.user_id);
    match form.action.as_str(){
        "save"=>{
            let project_name=sqlx::query_scalar::<_,String>("SELECT name FROM projects WHERE id=$1")
                .bind(form.project_id).fetch_optional(&st.db).await?.ok_or_else(WebError::not_found)?;
            if project_info(&project_name).is_none(){return Err(WebError::bad("Выберите один из шести ЖК"));}
            if !form.default_work.is_empty() && st.catalog.work_type(&form.default_work).is_none(){return Err(WebError::bad("Выберите вид работ из справочника"));}
            sqlx::query("INSERT INTO synology_project_mappings(source_name,project_id,default_work,changed_by) VALUES ($1,$2,$3,$4) ON CONFLICT(source_name) DO UPDATE SET project_id=EXCLUDED.project_id,default_work=EXCLUDED.default_work,changed_by=EXCLUDED.changed_by,changed_at=now()")
                .bind(source).bind(form.project_id).bind(&form.default_work).bind(&by).execute(&st.db).await?;
        },
        "remove"=>{sqlx::query("DELETE FROM synology_project_mappings WHERE source_name=$1")
            .bind(source).execute(&st.db).await?;},
        _=>return Err(WebError::bad("Неверная операция")),
    }
    repo::audit(&st.db,&by,"synology_mapping_changed",&format!("source={source} action={}",form.action)).await?;
    Ok(Redirect::to("/administration#synology-mapping"))
}
pub async fn project_image(Path(filename):Path<String>)->Response{
    let bytes:Option<&'static [u8]>=match filename.as_str(){
        "federation.webp"=>Some(include_bytes!("../assets/projects/federation.webp")),
        "okland.webp"=>Some(include_bytes!("../assets/projects/okland.webp")),
        "tetris.webp"=>Some(include_bytes!("../assets/projects/tetris.webp")),
        "kosmo.webp"=>Some(include_bytes!("../assets/projects/kosmo.webp")),
        "prime-tower.webp"=>Some(include_bytes!("../assets/projects/prime-tower.webp")),
        "duo.webp"=>Some(include_bytes!("../assets/projects/duo.webp")),
        _=>None
    };
    if let Some(bytes)=bytes {([(header::CONTENT_TYPE,"image/webp"),(header::CACHE_CONTROL,"public, max-age=86400")],bytes).into_response()}
    else{StatusCode::NOT_FOUND.into_response()}
}

#[cfg(test)]
mod employee_assignment_tests {
    use super::*;
    #[test]
    fn grant_requires_employee_active_and_matching_id() {
        let user=|id,active,kind|serde_json::json!({"result":[{"ID":id,"ACTIVE":active,"USER_TYPE":kind}]});
        assert!(eligible_staff_response(&user("88",true,"employee"),88));
        assert!(!eligible_staff_response(&user("88",false,"employee"),88));
        assert!(!eligible_staff_response(&user("88",true,"extranet"),88));
        assert!(!eligible_staff_response(&user("88",true,"email"),88));
        assert!(!eligible_staff_response(&user("88",true,"guest"),88));
        assert!(!eligible_staff_response(&user("88",true,"employee"),89));
        assert!(!eligible_staff_response(&serde_json::json!({"error":"ACCESS_DENIED"}),88));
    }
}
