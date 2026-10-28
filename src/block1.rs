//! Block 1: strictly scoped deliveries, quantitative checklist, linked evidence.
//! A linked file only supports the requirement within the SAME contractor delivery.
use crate::models::Document;
use anyhow::{Context,Result};
use serde_json::Value;
use sqlx::{PgPool,Row};
use std::collections::{HashMap,HashSet};

#[derive(Clone,Debug)]
pub struct Delivery {pub id:i64,pub revision:i32,pub prefix:String,pub expected_files:i64,pub manifest:Value}
fn from_delivery(id:i64,revision:i32,path:&str,manifest:&Value)->Result<Delivery>{
    let folder=path.rsplit('/').next().context("У поставки отсутствует папка Synology")?;
    crate::repo::validate_segment(folder)?;
    Ok(Delivery{id,revision,prefix:format!("ИД/{folder}/"),expected_files:manifest.as_array().map(|a|a.len() as i64).unwrap_or(0),manifest:manifest.clone()})
}
/// Only a published delivery (with NAS path) can become the active version.
pub async fn current_delivery(db:&PgPool,package:i64)->Result<Option<Delivery>>{
    let row=sqlx::query("SELECT id,revision,nas_path,manifest_json FROM contractor_submissions WHERE package_id=$1 AND nas_path IS NOT NULL AND status IN ('pending_review','checking','checked','rework') ORDER BY id DESC LIMIT 1")
        .bind(package).fetch_optional(db).await?;
    row.map(|r|from_delivery(r.get("id"),r.get("revision"),r.get::<String,_>("nas_path").as_str(),&r.get::<Value,_>("manifest_json"))).transpose()
}
pub async fn delivery_by_id(db:&PgPool,package:i64,id:i64)->Result<Option<Delivery>>{
    let row=sqlx::query("SELECT id,revision,nas_path,manifest_json FROM contractor_submissions WHERE package_id=$1 AND id=$2 AND nas_path IS NOT NULL")
        .bind(package).bind(id).fetch_optional(db).await?;
    row.map(|r|from_delivery(r.get("id"),r.get("revision"),r.get::<String,_>("nas_path").as_str(),&r.get::<Value,_>("manifest_json"))).transpose()
}
pub async fn delivery_for_run(db:&PgPool,run:i64)->Result<Option<Delivery>>{
    let row=sqlx::query("SELECT s.id,s.revision,s.nas_path,s.manifest_json FROM check_runs r JOIN contractor_submissions s ON s.id=r.submission_id WHERE r.id=$1 AND s.nas_path IS NOT NULL")
        .bind(run).fetch_optional(db).await?;
    row.map(|r|from_delivery(r.get("id"),r.get("revision"),r.get::<String,_>("nas_path").as_str(),&r.get::<Value,_>("manifest_json"))).transpose()
}
pub fn latest<'a>(docs:&'a [Document])->Vec<&'a Document>{
    let mut m:HashMap<&str,&Document>=HashMap::new();
    for d in docs {let entry=m.entry(d.relative_path.as_str()).or_insert(d);if d.version>entry.version || (d.version==entry.version && d.id>entry.id){*entry=d;}}
    m.into_values().filter(|d|d.is_present).collect()
}
pub fn is_delivery_id(d:&Document,delivery:Option<&Delivery>)->bool{
    d.is_present && d.source_kind=="ID" && delivery.map(|v|d.relative_path.starts_with(&v.prefix)).unwrap_or(true)
}
#[derive(Debug,Clone)]
pub struct SupportRequirement {pub id:i64,pub kind:String,pub title:String,pub basis:String,pub applies_when:String,pub min_count:i32,pub parent_document_id:Option<i64>}
pub async fn support_requirements(db:&PgPool,package:i64)->Result<Vec<SupportRequirement>>{
    let rows=sqlx::query("SELECT id,kind,title,basis,applies_when,min_count,parent_document_id FROM package_support_requirements WHERE package_id=$1 AND active=true ORDER BY kind,id")
        .bind(package).fetch_all(db).await?;
    rows.into_iter().map(|r|Ok(SupportRequirement{id:r.try_get("id")?,kind:r.try_get("kind")?,title:r.try_get("title")?,basis:r.try_get("basis")?,applies_when:r.try_get("applies_when")?,min_count:r.try_get("min_count")?,parent_document_id:r.try_get("parent_document_id")?})).collect()
}
pub async fn linked_evidence(db:&PgPool,package:i64)->Result<HashMap<i64,Vec<i64>>>{
    let rows=sqlx::query("SELECT r.id AS requirement_id,e.document_id FROM package_support_requirements r JOIN package_support_evidence e ON e.requirement_id=r.id WHERE r.package_id=$1 AND r.active=true")
        .bind(package).fetch_all(db).await?;
    let mut links:HashMap<i64,Vec<i64>>=HashMap::new();
    for row in rows {links.entry(row.try_get("requirement_id")?).or_default().push(row.try_get("document_id")?);}
    Ok(links)
}
pub async fn minimums(db:&PgPool,package:i64)->Result<HashMap<String,i32>>{
    let rows=sqlx::query("SELECT code,min_count FROM package_document_minimums WHERE package_id=$1")
        .bind(package).fetch_all(db).await?;
    let mut result=HashMap::new();
    for row in rows {result.insert(row.try_get("code")?,row.try_get("min_count")?);}
    Ok(result)
}
pub fn counted_codes<'a>(inputs:impl IntoIterator<Item=&'a Document>)->HashMap<String,usize>{
    let mut found=HashMap::new();
    for d in inputs {if let Some(code)=&d.catalog_code{*found.entry(code.clone()).or_insert(0)+=1;}}
    found
}
/// A work register is explicitly entered and approved by an engineer from actual
/// work volumes, PD/RD references and acceptance boundaries. One document type
/// may be needed multiple times on distinct locations or work stages.
#[derive(Clone,Debug)]
pub struct WorkOperation {pub id:i64,pub description:String,pub location:String,pub catalog_code:String,pub required_count:i32,pub basis:String}
pub async fn work_operations(db:&PgPool,package:i64)->Result<Vec<WorkOperation>>{
    let rows=sqlx::query("SELECT id,description,location,catalog_code,required_count,basis FROM package_work_operations WHERE package_id=$1 AND active=true ORDER BY id")
        .bind(package).fetch_all(db).await?;
    rows.into_iter().map(|r|Ok(WorkOperation{id:r.try_get("id")?,description:r.try_get("description")?,location:r.try_get("location")?,catalog_code:r.try_get("catalog_code")?,required_count:r.try_get("required_count")?,basis:r.try_get("basis")?})).collect()
}
/// Evidence linking an act to a specific work stage/zone; archived revisions are
/// filtered out at evaluation time using actual document IDs from that delivery.
pub async fn work_evidence(db:&PgPool,package:i64)->Result<HashMap<i64,Vec<i64>>>{
    let rows=sqlx::query("SELECT w.id AS operation_id,e.document_id FROM package_work_operations w JOIN package_work_evidence e ON e.operation_id=w.id WHERE w.package_id=$1 AND w.active=true")
        .bind(package).fetch_all(db).await?;
    let mut out:HashMap<i64,Vec<i64>>=HashMap::new();
    for row in rows {out.entry(row.try_get("operation_id")?).or_default().push(row.try_get("document_id")?);}
    Ok(out)
}
pub fn work_evidence_count(work:&WorkOperation,links:&HashMap<i64,Vec<i64>>,inputs:&[&Document])->usize {
    let mut seen=HashSet::new();
    if let Some(ids)=links.get(&work.id){
        for id in ids {
            if let Some(doc)=inputs.iter().find(|d|d.id==*id && d.catalog_code.as_deref()==Some(work.catalog_code.as_str())){
                seen.insert(doc.sha256.to_lowercase());
            }
        }
    }
    seen.len()
}
/// Existing checklist minima and quantities derived from registered activities
/// are NOT added together: the register is a refinement of the baseline.
pub fn effective_minimums(base:&HashMap<String,i32>,operations:&[WorkOperation])->HashMap<String,i32>{
    let mut out=base.clone();
    let mut measured:HashMap<String,i32>=HashMap::new();
    for work in operations {
        let amount=measured.entry(work.catalog_code.clone()).or_insert(0);
        *amount=amount.saturating_add(work.required_count);
    }
    for (code,count) in measured {
        let prior=out.get(&code).copied().unwrap_or(1);
        out.insert(code,prior.max(count));
    }
    out
}
/// Duplicate content under different names is a single piece of evidence.
pub fn counted_unique_codes<'a>(inputs:impl IntoIterator<Item=&'a Document>)->HashMap<String,usize>{
    let mut seen:HashSet<(String,String)>=HashSet::new();
    let mut out=HashMap::new();
    for d in inputs {
        if let Some(code)=&d.catalog_code {
            let sha=d.sha256.to_lowercase();
            if seen.insert((code.clone(),sha)) { *out.entry(code.clone()).or_insert(0)+=1; }
        }
    }
    out
}
/// Each manifest entry must be present and match the immutable original SHA.
/// Metadata without file paths is not treated as authoritative evidence.
pub fn manifest_problems(scope:&Delivery,inputs:&[&Document])->Vec<String>{
    let mut bad=Vec::new();
    if let Some(entries)=scope.manifest.as_array(){
        for entry in entries {
            let Some(relative)=entry.get("path").and_then(Value::as_str) else {bad.push("В описи поставки не указан путь файла".to_string());continue};
            let Some(sha)=entry.get("sha256").and_then(Value::as_str) else {bad.push(format!("Нет SHA-256 в описи: {relative}"));continue};
            let full=format!("{}{relative}",scope.prefix);
            match inputs.iter().find(|d|d.relative_path==full){
                None=>bad.push(format!("Не зарегистрирован файл из описи: {relative}")),
                Some(d) if !d.sha256.eq_ignore_ascii_case(sha)=>bad.push(format!("SHA-256 не совпадает с описью: {relative}")),
                _=>(),
            }
        }
    }
    bad
}
/// Act-specific attachments must be bound to the same current delivery.
pub fn requirement_parent_valid(requirement:&SupportRequirement,inputs:&[&Document])->bool{
    if requirement.kind=="attachment" && requirement.parent_document_id.is_none(){return false;}
    parent_in_delivery(requirement.parent_document_id,inputs)
}
pub fn parent_in_delivery(parent:Option<i64>,inputs:&[&Document])->bool{
    parent.map(|id|inputs.iter().any(|d|d.id==id)).unwrap_or(true)
}
/// No percentage can certify a set with unknown document types, failed OCR or
/// a broken manifest; these are blocked and separately explained in findings.
pub fn complete_metadata(inputs:&[&Document])->bool{
    !inputs.is_empty() && inputs.iter().all(|d|d.catalog_code.is_some() &&
        matches!(d.recognition_status.as_str(),"extracted"|"ocr"|"ocr_vibe"|"ocr_local") &&
        d.recognition_confidence.map_or(true,|v|v>=0.7))
}

/// No submission should be marked complete just because a document from an old revision matches.
pub fn evidence_count(req_id:i64,links:&HashMap<i64,Vec<i64>>,inputs:&[&Document])->usize{
    let Some(ids)=links.get(&req_id) else{return 0};
    let mut hashes=HashSet::new();
    for id in ids {
        if let Some(doc)=inputs.iter().find(|d|d.id==*id) {
            hashes.insert(doc.sha256.to_lowercase());
        }
    }
    hashes.len()
}
#[cfg(test)]mod tests {
    use super::*;
    fn doc(id:i64,rel:&str,code:Option<&str>)->Document{
        Document{id,package_id:10,relative_path:rel.to_owned(),source_kind:"ID".to_owned(),origin:"synology".to_owned(),is_present:true,
            sha256:format!("hash{id}"),version:1,archive_path:format!("archive-{id}"),draft_path:None,
            document_type:None,catalog_code:code.map(str::to_owned),recognized_text:String::new(),recognition_status:"extracted".into(),
            recognition_confidence:Some(1.0),spelling_notes_json:"[]".into()}
    }
    #[test]fn checks_only_current_delivery_and_count(){
        let v1=doc(1,"ИД/Поставка-00001/a.doc",Some("1.4"));
        let v2=doc(2,"ИД/Земляные работы-версия-7-28.10.26/a.doc",Some("1.4"));
        let v3=doc(3,"ИД/Земляные работы-версия-7-28.10.26/b.doc",Some("1.4"));
        let scope=from_delivery(7,2,"/ResData/ИД/Земляные работы-версия-7-28.10.26",&serde_json::json!([])).unwrap();
        let docs=vec![v1,v2,v3];
        let active=docs.iter().filter(|d|is_delivery_id(d,Some(&scope))).collect::<Vec<_>>();
        assert_eq!(active.len(),2);
        assert_eq!(counted_codes(active.iter().copied()).get("1.4"),Some(&2usize));
        let links=HashMap::from([(21i64,vec![1i64,3i64])]);
        assert_eq!(evidence_count(21,&links,&active),1); // old revision cannot satisfy a requirement
    }
    #[test]fn work_volumes_are_summed_per_code_without_double_counting_baseline(){
        let base=HashMap::from([("1.4".to_string(),2)]);
        let works=vec![WorkOperation{id:1,description:"траншея А".into(),location:"А".into(),catalog_code:"1.4".into(),required_count:3,basis:"РД лист 1".into()},
                       WorkOperation{id:2,description:"траншея Б".into(),location:"Б".into(),catalog_code:"1.4".into(),required_count:4,basis:"РД лист 2".into()}];
        assert_eq!(effective_minimums(&base,&works).get("1.4"),Some(&7));
    }
    #[test]fn work_evidence_must_be_same_zone_type_and_revision(){
        let op=WorkOperation{id:15,description:"Траншея Б".into(),location:"ось Б".into(),catalog_code:"1.4".into(),required_count:2,basis:"РД лист 3".into()};
        let x=doc(1,"ИД/Доставка/a.doc",Some("1.4"));
        let wrong=doc(2,"ИД/Доставка/p.pdf",Some("1.5"));
        let links=HashMap::from([(15i64,vec![1i64,2i64,3i64])]);
        assert_eq!(work_evidence_count(&op,&links,&[&x,&wrong]),1);
        assert_eq!(work_evidence_count(&op,&links,&[&wrong]),0);
    }
    #[test]fn refuses_duplicate_files_and_wrong_sha(){
        let x=doc(1,"ИД/Доставка/a.doc",Some("1.4"));
        let mut y=doc(2,"ИД/Доставка/b.doc",Some("1.4"));y.sha256=x.sha256.clone();
        assert_eq!(counted_unique_codes([&x,&y]).get("1.4"),Some(&1));
        let delivery=from_delivery(1,1,"/ResData/ИД/Доставка",&serde_json::json!([{"path":"a.doc","sha256":"hash1"},{"path":"b.doc","sha256":"invalid"}])).unwrap();
        assert_eq!(manifest_problems(&delivery,&[&x,&y]).len(),1);
        assert!(!parent_in_delivery(Some(999),&[&x]));
    }
    #[test]fn incomplete_recognition_cannot_be_green(){
        let mut d=doc(1,"ИД/Доставка/a.doc",Some("1.4"));
        assert!(complete_metadata(&[&d]));d.recognition_status="error".into();
        assert!(!complete_metadata(&[&d]));
    }
    #[test]fn templates_cover_full_two_appendices(){
        assert_eq!(SUPPORT_TEMPLATES.len(),31);
        assert_eq!(SUPPORT_TEMPLATES.iter().filter(|t|t.kind=="supplier").count(),19);
        assert_eq!(SUPPORT_TEMPLATES.iter().filter(|t|t.kind=="laboratory").count(),12);
    }
    #[test]fn delivery_prefix_is_bounded(){
        let a=from_delivery(7,2,"/ResData/DUO/ИД/Земляные работы-версия-7-28.10.26",&serde_json::json!([{},{}])).unwrap();
        assert_eq!(a.prefix,"ИД/Земляные работы-версия-7-28.10.26/");
        assert_eq!(a.expected_files,2);
        assert!(from_delivery(8,1,"/ResData/ИД/..",&serde_json::json!([])).is_err());
    }
}

/// Suggested requirement types are informational, NOT automatically mandatory.
/// Engineer must select applicability for a particular project and delivery.
#[derive(Clone,Copy)]
pub struct SupportTemplate {pub code:&'static str,pub kind:&'static str,pub title:&'static str,pub applies_when:&'static str,pub basis:&'static str}
pub const SUPPORT_TEMPLATES:&[SupportTemplate]=&[
    SupportTemplate{code:"S01",kind:"supplier",title:"Сертификат качества металлопроката",applies_when:"При использовании металлопроката",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 1"},
    SupportTemplate{code:"S02",kind:"supplier",title:"Документ о качестве бетонной смеси или раствора, включая удельную активность радионуклидов",applies_when:"При применении бетонных и растворных смесей",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 2"},
    SupportTemplate{code:"S03",kind:"supplier",title:"Паспорт качества сыпучих материалов, включая удельную активность радионуклидов",applies_when:"При применении песка, щебня, гравия, керамзита",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 3"},
    SupportTemplate{code:"S04",kind:"supplier",title:"Сертификат/декларация и пожарная безопасность кабельно-проводниковой продукции",applies_when:"При использовании кабелей и проводов",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 4"},
    SupportTemplate{code:"S05",kind:"supplier",title:"Паспорт качества партии каменных штучных материалов и протоколы производителя (при наличии)",applies_when:"При использовании кирпичей, блоков, перемычек",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 5"},
    SupportTemplate{code:"S06",kind:"supplier",title:"Сертификат/декларация кабеленесущих конструкций и пожарная безопасность",applies_when:"При применении лотков, каналов, гофр",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 6"},
    SupportTemplate{code:"S07",kind:"supplier",title:"Паспорт качества воздуховодов с толщиной металла, декларация и пожарная безопасность",applies_when:"При монтаже воздуховодов",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 7"},
    SupportTemplate{code:"S08",kind:"supplier",title:"Документ о соответствии крепёжных элементов или отказное письмо",applies_when:"При использовании крепёжных элементов",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 8"},
    SupportTemplate{code:"S09",kind:"supplier",title:"Сертификаты, пожарная и санитарная безопасность противопожарных герметизирующих материалов",applies_when:"При использовании противопожарных герметизирующих материалов",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 9"},
    SupportTemplate{code:"S10",kind:"supplier",title:"Сертификат/декларация и санитарное соответствие герметиков, пен, лент",applies_when:"При использовании герметиков, пен, лент",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 10"},
    SupportTemplate{code:"S11",kind:"supplier",title:"Документ качества стальных сварных конструкций",applies_when:"При наличии металлических изделий со сварными соединениями",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 11"},
    SupportTemplate{code:"S12",kind:"supplier",title:"Паспорт, инструкция и сертификаты оборудования/приборов",applies_when:"При применении оборудования и приборов",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 12"},
    SupportTemplate{code:"S13",kind:"supplier",title:"Документ качества пиломатериалов",applies_when:"При использовании пиломатериалов",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 13"},
    SupportTemplate{code:"S14",kind:"supplier",title:"Сертификат/декларация стоек, подвесов, профилей и подтверждение пожарных/санитарных требований",applies_when:"При использовании стоек, подвесов, профилей",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 14"},
    SupportTemplate{code:"S15",kind:"supplier",title:"Сертификаты и подтверждение пожарных/санитарных требований ГКЛ/ГВЛ",applies_when:"При использовании ГКЛ/ГВЛ",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 15"},
    SupportTemplate{code:"S16",kind:"supplier",title:"Сертификаты и подтверждение пожарных/санитарных требований отделочных материалов",applies_when:"При использовании отделочных материалов",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 16"},
    SupportTemplate{code:"S17",kind:"supplier",title:"Сертификаты и подтверждение пожарных/санитарных требований пластиковых труб/фитингов",applies_when:"При применении пластиковых труб и фитингов",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 17"},
    SupportTemplate{code:"S18",kind:"supplier",title:"Паспорт, сертификат и санитарное соответствие запорной арматуры",applies_when:"При применении запорной арматуры",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 18"},
    SupportTemplate{code:"S19",kind:"supplier",title:"Паспорт и сертификаты декоративных архитектурных отделочных элементов",applies_when:"При применении декоративных архитектурных элементов",basis:"Блок 1 ТЗ, перечень сопроводительной документации поставщиков, пункт 19"},
    SupportTemplate{code:"L01",kind:"laboratory",title:"Протоколы статического/динамического испытания свай",applies_when:"При наличии свайного поля",basis:"Блок 1 ТЗ, перечень лабораторных испытаний, пункт 1"},
    SupportTemplate{code:"L02",kind:"laboratory",title:"Протоколы определения плотности уплотнённого грунта",applies_when:"При производстве земляных работ с уплотнением",basis:"Блок 1 ТЗ, перечень лабораторных испытаний, пункт 2"},
    SupportTemplate{code:"L03",kind:"laboratory",title:"Протоколы прочности бетона, 7 и 28 суток, НК и при необходимости морозостойкости/водонепроницаемости",applies_when:"При устройстве монолитных железобетонных конструкций",basis:"Блок 1 ТЗ, перечень лабораторных испытаний, пункт 3"},
    SupportTemplate{code:"L04",kind:"laboratory",title:"Протоколы испытания раствора на сжатие",applies_when:"При устройстве ограждающих конструкций и перегородок с раствором",basis:"Блок 1 ТЗ, перечень лабораторных испытаний, пункт 4"},
    SupportTemplate{code:"L05",kind:"laboratory",title:"Испытания крепежа фасада, горючести материалов и огнестойкости конструкций при применимости",applies_when:"При устройстве фасадных систем",basis:"Блок 1 ТЗ, перечень лабораторных испытаний, пункт 5"},
    SupportTemplate{code:"L06",kind:"laboratory",title:"Электротехнический отчёт, протоколы испытаний и измерений",applies_when:"При электромонтажных работах",basis:"Блок 1 ТЗ, перечень лабораторных испытаний, пункт 6"},
    SupportTemplate{code:"L07",kind:"laboratory",title:"Протоколы работоспособности СОУЭ, огнестойкости соединительных линий и аварийного освещения",applies_when:"При устройстве СОУЭ",basis:"Блок 1 ТЗ, перечень лабораторных испытаний, пункт 7"},
    SupportTemplate{code:"L08",kind:"laboratory",title:"Протоколы аэродинамических испытаний противодымной вентиляции, обследования каналов",applies_when:"При устройстве вентиляции и дымоудаления",basis:"Блок 1 ТЗ, перечень лабораторных испытаний, пункт 8"},
    SupportTemplate{code:"L09",kind:"laboratory",title:"Протоколы качества питьевой воды и водоотдачи наружного пожарного водопровода",applies_when:"При устройстве наружного водоснабжения",basis:"Блок 1 ТЗ, перечень лабораторных испытаний, пункт 9"},
    SupportTemplate{code:"L10",kind:"laboratory",title:"Протоколы водоотдачи внутреннего противопожарного водопровода",applies_when:"При устройстве внутреннего водоснабжения",basis:"Блок 1 ТЗ, перечень лабораторных испытаний, пункт 10"},
    SupportTemplate{code:"L11",kind:"laboratory",title:"Протоколы дозы гамма-излучения, радона/торона и санитарно-гигиенические заключения",applies_when:"При внутренней отделке (по применимым требованиям)",basis:"Блок 1 ТЗ, перечень лабораторных испытаний, пункт 11"},
    SupportTemplate{code:"L12",kind:"laboratory",title:"Протоколы испытания металлических ограждений",applies_when:"При устройстве металлических ограждений лестниц, балконов, крыш",basis:"Блок 1 ТЗ, перечень лабораторных испытаний, пункт 12"},
];
