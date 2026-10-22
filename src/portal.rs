//! Single-app project and work-type UX. All role decisions are in access.rs.
use crate::{access::{self,Principal},models::Package,repo,web::{self,Shared,WebError}};
use axum::{extract::{Path,State},Extension,Form,response::{Html,Redirect,Response,IntoResponse},http::{header,StatusCode}};
use serde::Deserialize;
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
fn photo(slug:&str)->String {format!("/assets/projects/{slug}.webp")}
fn work_icon(index:usize)->(&'static str,&'static str){
    const ICONS:[(&str,&str);14]=[
      ("⚒","blue"),("▦","coral"),("◉","purple"),("♧","cyan"),("◎","mint"),
      ("♧","blue"),("◌","green"),("ϟ","orange"),("⚡","violet"),
      ("♨","red"),("⬡","red"),("▣","cyan"),("▤","amber"),("♧","green")
    ]; ICONS[index.min(13)]
}
fn employee_picker()->&'static str {include_str!("../assets/employee-picker.html")}

pub async fn projects(State(st):State<Shared>,Extension(user):Extension<Principal>)->Result<Html<String>,WebError>{
    let mut cards=String::new();let mut available=0usize;
    for (name,address,slug) in PROJECTS {
        let project=sqlx::query("SELECT id FROM projects WHERE name=$1").bind(name).fetch_optional(&st.db).await?;
        let Some(project)=project else {continue};
        let id:i64=project.try_get("id")?;
        if !access::project_visible(&st,&user,id).await {continue;}
        available+=1;
        cards.push_str(&format!(r#"<a href="/projects/{id}" class="project-card" aria-label="Открыть объект {name}">
          <img src="{}" alt="Визуализация ЖК {}" loading="lazy">
          <div class="project-desc"><strong>{}</strong><small>⌖ {}</small><span class="card-arrow">➜</span></div>
        </a>"#,photo(slug),h(name),h(name),h(address)));
    }
    let empty=if available==0 {"<p class='empty-work'>Вам пока не назначены объекты строительства. Обратитесь к администратору проекта.</p>"}else{""};
    let body=format!(r#"<div class="page-title"><h1>Исполнительная документация</h1><p>Выберите объект строительства и перейдите к комплектам исполнительной документации.</p></div>
      <section class="white-panel" id="projects"><div class="panel-heading"><h2><span class="section-icon">▤</span> Выбор объекта строительства</h2><div class="panel-tools"><input type="search" id="project-search" placeholder="⌕  Поиск по объектам…" aria-label="Поиск по объектам"><span class="pill">{available} объектов</span></div></div>
      <div class="projects-grid">{cards}</div>{empty}</section>
      <section class="selection-panel"><div class="selection-note"><strong>Выберите жилой комплекс</strong><small>Откроются доступные для вас виды работ и комплекты ИД.</small></div><span>→</span></section>
      <script>document.getElementById('project-search')?.addEventListener('input',e=>{{const q=e.target.value.toLocaleLowerCase('ru');document.querySelectorAll('.project-card').forEach(c=>c.hidden=!c.textContent.toLocaleLowerCase('ru').includes(q));}});</script>"#);
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
    for (idx,wt) in st.catalog.work_types.iter().enumerate(){
        let existing=pkgs.iter().find(|p|p.name==wt.name);
        let allowed=if let Some(p)=existing {access::package_role(&st,&user,p.id).await.is_some()}else{false};
        if !manager && !allowed {continue;}
        if existing.is_some(){active+=1;}
        let (symbol,tone)=work_icon(idx);
        if let Some(pkg)=existing {
            let files:(i64,)=sqlx::query_as("SELECT count(*) FROM documents WHERE package_id=$1 AND is_present=true")
                .bind(pkg.id).fetch_one(&st.db).await?;
            works.push_str(&format!(r#"<a class="work-card" href="/packages/{}"><span class="work-symbol {}">{}</span><div><strong>{}</strong><small>Комплект ИД · {} файлов</small></div><span class="work-arrow">›</span></a>"#,pkg.id,tone,symbol,h(&wt.name),files.0));
            if manager {assigned_table.push_str(&format!(r#"<tr><td><strong>{}</strong></td><td><form class="assign-form" method="post" action="/projects/{id}/assign">{}<input type="hidden" name="package_id" value="{}"><input type="number" min="1" name="user_id" required placeholder="ID сотрудника" class="employee-id" aria-label="ID сотрудника"><select class="employee-picker" aria-label="Выбрать сотрудника"><option value="">Из списка Bitrix24…</option></select><select name="role"><option value="uploader">Пользователь (загрузка)</option><option value="engineer">Инженер ПТО</option><option value="viewer">Просмотр</option><option value="remove">Снять доступ</option></select><button>Применить</button></form></td></tr>"#,h(&wt.name),web::hidden(&st.csrf),pkg.id));}
        } else if manager {
            works.push_str(&format!(r#"<form method="post" action="/projects/{id}/works" class="work-card work-add">{}<input type="hidden" name="work_type" value="{}"><span class="work-symbol {}">{}</span><div><strong>{}</strong><small>Ещё не создан · нажмите для добавления</small></div><button title="Создать комплект">＋</button></form>"#,web::hidden(&st.csrf),h(&wt.name),tone,symbol,h(&wt.name)));
        }
    }
    let bulk=if manager {format!(r#"<form method="post" action="/projects/{id}/works/all" class="inline">{}<button class="primary">＋ Создать все 14 видов работ</button></form>"#,web::hidden(&st.csrf))} else {String::new()};
    let assignment=if manager {format!(r#"<details class="admin-management"><summary>Управление доступом сотрудников к видам работ</summary><div class="section-copy">Выберите сотрудника из списка Bitrix24 или введите его числовой ID. Назначение влияет на серверные права, а не только на отображение карточек.</div><table><thead><tr><th>Вид работ</th><th>Назначить сотрудника</th></tr></thead><tbody>{assigned_table}</tbody></table>{}</details>"#,employee_picker())}else{String::new()};
    let empty=if works.is_empty() {"<p class='empty-work'>Доступных видов работ пока нет. Администратор проекта должен создать комплект и назначить вам доступ.</p>"}else{""};
    let body=format!(r#"<div class="crumb"><a href="/">← К выбору объектов</a> <span>›</span> {}</div>
      <section class="project-hero" style="--hero-image:url('{}')"><div class="project-hero-content"><h1>{}</h1><p>⌖ {}, Астрахань</p><div class="hero-stats"><span>▤ Жилой комплекс</span><span>▥ Исполнительная документация</span><span>◫ {active} из 14 разделов</span></div></div></section>
      <div class="section-heading"><div><h2>Комплекты исполнительной документации</h2><p>Выберите раздел для просмотра и проверки документов.</p></div><span class="counter">{active} разделов создано</span>{bulk}</div>
      <div class="works-grid">{works}</div>{empty}{assignment}"#,h(&name),photo(slug),h(&name),h(address));
    Ok(web::layout(&name,body))
}

#[derive(Deserialize)]pub struct WorkForm{pub csrf:String,pub work_type:String}
#[derive(Deserialize)]pub struct CsrfForm{pub csrf:String}
#[derive(Deserialize)]pub struct AssignmentForm{pub csrf:String,pub package_id:i64,pub user_id:i64,pub role:String}
#[derive(Deserialize)]pub struct ManagerForm{pub csrf:String,pub project_id:i64,pub user_id:i64,pub action:String}

pub async fn create_work(State(st):State<Shared>,Extension(actor):Extension<Principal>,Path(id):Path<i64>,Form(form):Form<WorkForm>)->Result<Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    if !access::is_manager(&st,&actor,id).await{return Err(WebError::forbidden());}
    let wt=st.catalog.work_type(&form.work_type).ok_or_else(||WebError::bad("Выберите вид работ из справочника"))?;
    let project_name=sqlx::query_scalar::<_,String>("SELECT name FROM projects WHERE id=$1")
        .bind(id).fetch_optional(&st.db).await?.ok_or_else(WebError::not_found)?;
    if project_info(&project_name).is_none(){return Err(WebError::not_found());}
    let pkg=repo::get_or_create_package(&st.db,id,&wt.name).await?;
    sqlx::query("UPDATE packages SET catalog_name=$1 WHERE id=$2 AND catalog_name IS NULL").bind(&wt.name).bind(pkg.id).execute(&st.db).await?;
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
    repo::audit(&st.db,&format!("bitrix:{}:{}",actor.member,actor.user_id),"create_all_works",&format!("project={id}")).await?;
    Ok(Redirect::to(&format!("/projects/{id}")))
}
pub async fn assign_work(State(st):State<Shared>,Extension(actor):Extension<Principal>,Path(id):Path<i64>,Form(form):Form<AssignmentForm>)->Result<Redirect,WebError>{
    web::verify(&form.csrf,&st)?;
    if !access::is_manager(&st,&actor,id).await{return Err(WebError::forbidden());}
    if form.user_id<1{return Err(WebError::bad("Некорректный ID сотрудника"));}
    let belongs=sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM packages WHERE id=$1 AND project_id=$2)")
        .bind(form.package_id).bind(id).fetch_one(&st.db).await?;
    if !belongs{return Err(WebError::bad("Комплект не относится к этому объекту"));}
    if form.role=="remove"{
        sqlx::query("DELETE FROM bitrix_package_acl WHERE member_id=$1 AND user_id=$2 AND package_id=$3")
          .bind(&actor.member).bind(form.user_id).bind(form.package_id).execute(&st.db).await?;
    }else{
        if !["uploader","viewer","engineer"].contains(&form.role.as_str()) {return Err(WebError::bad("Недопустимая роль"));}
        sqlx::query("INSERT INTO bitrix_principals(member_id,user_id) VALUES ($1,$2) ON CONFLICT(member_id,user_id) DO NOTHING")
            .bind(&actor.member).bind(form.user_id).execute(&st.db).await?;
        sqlx::query("INSERT INTO bitrix_package_acl(member_id,user_id,package_id,role) VALUES ($1,$2,$3,$4) ON CONFLICT(member_id,user_id,package_id) DO UPDATE SET role=EXCLUDED.role")
            .bind(&actor.member).bind(form.user_id).bind(form.package_id).bind(&form.role).execute(&st.db).await?;
    }
    repo::audit(&st.db,&format!("bitrix:{}:{}",actor.member,actor.user_id),"assign_work",&format!("project={id} package={} user={} role={}",form.package_id,form.user_id,form.role)).await?;
    Ok(Redirect::to(&format!("/projects/{id}")))
}

pub async fn administration(State(st):State<Shared>,Extension(actor):Extension<Principal>)->Result<Html<String>,WebError>{
    if !actor.superuser{return Err(WebError::forbidden());}
    let managers=sqlx::query("SELECT m.user_id,p.name FROM bitrix_project_managers m JOIN projects p ON p.id=m.project_id WHERE m.member_id=$1 ORDER BY p.name,m.user_id")
        .bind(&actor.member).fetch_all(&st.db).await?;
    let mut trs=String::new();for r in managers{let user_id:i64=r.try_get("user_id")?;let name:String=r.try_get("name")?;trs.push_str(&format!("<tr><td>{}</td><td>{user_id}</td></tr>",h(&name)));}
    let mut options=String::new();for (name,_,_) in PROJECTS {
        let row=sqlx::query("SELECT id FROM projects WHERE name=$1").bind(name).fetch_one(&st.db).await?;
        let id:i64=row.try_get("id")?;
        options.push_str(&format!("<option value='{id}'>{}</option>",h(name)));
    }
    let body=format!(r#"<div class="page-title"><h1>Администрирование</h1><p>Только для супер-администраторов Bitrix24 с ID 1 и 336.</p></div>
    <section class="white-panel"><h2>Администраторы проектов</h2><p>Назначенный администратор создаёт виды работ и распределяет доступ сотрудников только внутри своего ЖК.</p>
    <form class="admin-form" method="post" action="/administration/managers">{}<label>Проект<select name="project_id">{options}</select></label><label>ID сотрудника<input name="user_id" class="employee-id" type="number" min="1" required placeholder="ID сотрудника"></label><label>Сотрудник из Bitrix24<select class="employee-picker"><option value="">Из списка Bitrix24…</option></select></label><label>Действие<select name="action"><option value="grant">Назначить</option><option value="revoke">Снять назначение</option></select></label><button>Сохранить</button></form>
    <table><thead><tr><th>Жилой комплекс</th><th>ID администратора проекта</th></tr></thead><tbody>{trs}</tbody></table>{}</section>
    <section class="white-panel"><h2>Роли и ограничения</h2><p><strong>Супер-администратор</strong>: Bitrix24 ID 1 и 336. <strong>Администратор проекта</strong>: назначается на выбранный ЖК. <strong>Пользователь</strong>: видит только назначенные комплекты ИД.</p><p>Все права проверяются на сервере, включая скачивание файлов, OCR и отправку форм.</p></section>"#,web::hidden(&st.csrf),employee_picker());
    Ok(web::layout("Администрирование",body))
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
        sqlx::query("INSERT INTO bitrix_principals(member_id,user_id) VALUES ($1,$2) ON CONFLICT(member_id,user_id) DO NOTHING")
            .bind(&actor.member).bind(form.user_id).execute(&st.db).await?;
        sqlx::query("INSERT INTO bitrix_project_managers(member_id,user_id,project_id,granted_by) VALUES ($1,$2,$3,$4) ON CONFLICT(member_id,user_id,project_id) DO UPDATE SET granted_by=EXCLUDED.granted_by,granted_at=now()")
            .bind(&actor.member).bind(form.user_id).bind(form.project_id).bind(actor.user_id).execute(&st.db).await?;
    }else{return Err(WebError::bad("Недопустимая операция"));}
    repo::audit(&st.db,&format!("bitrix:{}:{}",actor.member,actor.user_id),"manager_assignment",&format!("project={} user={} {}",form.project_id,form.user_id,form.action)).await?;
    Ok(Redirect::to("/administration"))
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
