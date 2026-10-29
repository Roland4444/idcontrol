//! MVP rules: absence of evidence NEVER implies compliance.
use crate::{block1, project_requirements, catalog::Catalog, models::{Document, FindingInput}, norms, repo};
use anyhow::Result;
use chrono::NaiveDate;
use once_cell::sync::Lazy;
use regex::Regex;
use sqlx::PgPool;
use std::collections::HashSet;

static ANY_DATE:Lazy<Regex>=Lazy::new(||Regex::new(r"\b([0-3]?\d)[.\-/]([01]?\d)[.\-/](20\d{2})\b").unwrap());
static ACT:Lazy<Regex>=Lazy::new(||Regex::new(r"(?i)(?:дата\s+(?:составления\s+)?акта|акт\s*№.{0,35}?от)\s*[:\-]?\s*([0-3]?\d[.]\d{1,2}[.]20\d{2})").unwrap());
static PROTOCOL:Lazy<Regex>=Lazy::new(||Regex::new(r"(?i)протокол[^\n]{0,110}?(?:от|дата)\s*[:\-]?\s*([0-3]?\d[.]\d{1,2}[.]20\d{2})").unwrap());

pub fn timeline_discrepancies(text:&str)->Vec<String>{
    let mut result=HashSet::new();
    let acts:Vec<_>=ACT.captures_iter(text).filter_map(|cap|NaiveDate::parse_from_str(&cap[1],"%d.%m.%Y").ok().map(|d|(cap[1].to_string(),d))).collect();
    let prot:Vec<_>=PROTOCOL.captures_iter(text).filter_map(|cap|NaiveDate::parse_from_str(&cap[1],"%d.%m.%Y").ok().map(|d|(cap[1].to_string(),d))).collect();
    for (at,a) in &acts{for (pt,p) in &prot{if p>a {
        result.insert(format!("Дата протокола {pt} позже даты акта {at}. Проверить ссылку, применимость и исключения."));
    }}}
    let mut all:Vec<_>=result.into_iter().collect(); all.sort();all
}
pub async fn analyze(db:&PgPool, catalog:&Catalog,package_id:i64,source:&str)->Result<i64>{
    let delivery=block1::current_delivery(db,package_id).await?;
    analyze_internal(db,catalog,package_id,source,delivery.as_ref()).await
}
/// Evaluate exactly one contractor delivery instead of accidentally combining
/// every archived revision of the same package. Reference PD/RD remains shared.
pub async fn analyze_delivery(db:&PgPool,catalog:&Catalog,package_id:i64,source:&str,delivery_id:i64,delivery_prefix:&str)->Result<i64>{
    let scope=block1::delivery_by_id(db,package_id,delivery_id).await?
        .ok_or_else(||anyhow::anyhow!("Поставка не найдена в реестре"))?;
    if scope.prefix!=delivery_prefix {anyhow::bail!("Несоответствие идентификатора и папки поставки");}
    analyze_internal(db,catalog,package_id,source,Some(&scope)).await
}
async fn analyze_internal(db:&PgPool,catalog:&Catalog,package_id:i64,source:&str,delivery:Option<&block1::Delivery>)->Result<i64>{
    let package=repo::package(db,package_id).await?;
    let docs=repo::documents(db,package_id).await?;
    let latest=block1::latest(&docs);
    // No published contractor revision means that files from old or failed
    // revisions must not silently satisfy today's official submission.
    let has_delivery_history:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM contractor_submissions WHERE package_id=$1)")
        .bind(package_id).fetch_one(db).await?;
    let inputs:Vec<&Document>=if has_delivery_history && delivery.is_none(){Vec::new()}else{
        latest.iter().filter(|d|block1::is_delivery_id(d,delivery)).copied().collect()
    };
    // An uploaded normative act, SRO extract or order is NOT a design drawing.
    // Preserve legacy REFERENCE paths as valid PD/RD while distinguishing new
    // reference categories introduced in v0.9.19.
    let reference_exists=latest.iter().any(|d|{
        if !d.is_present || d.source_kind!="REFERENCE" {return false;}
        let segment=d.relative_path.strip_prefix("REFERENCE/")
            .and_then(|p|p.split('/').next()).unwrap_or("");
        !matches!(segment,"NTD"|"ORDERS"|"SRO"|"PERMIT"|"QUALITY"|"JOURNALS"|"OTHER")
    });
    let mut findings=Vec::new();
    let norm_rules=norms::approved_rules(db,package.catalog_name.as_deref().unwrap_or("")).await?;
    // Results are snapshotted at run time, so revoking a rule does not erase history.
    let mut norm_results:Vec<(i64,i64,i32,String,String,String,String)>=Vec::new();
    let mut matched_rules=0usize;
    if has_delivery_history && delivery.is_none() {
        findings.push(FindingInput::new(1,"NO-PUBLISHED-DELIVERY","Пока нет опубликованной поставки ИД. Старые и неуспешные загрузки не могут подтверждать комплектность."));
    }
    if inputs.is_empty(){
        let message=if delivery.is_some() {
            "В выбранной поставке ИД не зарегистрированы. Проверьте импорт с Synology, привязку к виду работ и категорию ID (это ещё не доказательство отсутствия файлов у подрядчика)."
        } else {"В комплекте не найдены зарегистрированные документы ИД. Проверьте папку и результаты сканирования."};
        findings.push(FindingInput::new(1,"E01",message).mismatch());
    }
    if let Some(scope)=delivery {
        for (index,problem) in block1::manifest_problems(scope,&inputs).into_iter().take(30).enumerate(){
            findings.push(FindingInput::new(1,format!("MANIFEST-{}",index+1),problem).mismatch());
        }
        if scope.expected_files>0 && (inputs.len() as i64)<scope.expected_files {
            findings.push(FindingInput::new(1,"IMPORT-GAP",format!("В поставке по описи {} файлов, зарегистрировано как ИД: {}. Проверьте импорт, поддерживаемые форматы и классификацию, прежде чем оценивать комплектность.",scope.expected_files,inputs.len())));
        }
    }
    let mut required:Vec<String>=serde_json::from_str(&package.required_codes_json).unwrap_or_default();
    let project_state=project_requirements::checklist(db,package_id).await?;
    let mut project_basis=std::collections::HashMap::new();
    // Candidates are merely evidence until the engineer signs off against the
    // exact current project-source revisions. Never import draft decisions.
    if project_state.candidates>0 {
        if !project_state.signed {
            findings.push(FindingInput::new(1,"PROJECT-REQUIREMENTS-PENDING",
                format!("Проектные требования из КР/ПЗУ/ПОС/ППР не утверждены для текущих редакций. Требуют решения: {}. Проверьте страницу «Требования из проекта».",project_state.pending)));
        } else {
            for item in &project_state.approved {
                if !required.contains(&item.code){required.push(item.code.clone());}
                project_basis.entry(item.code.clone()).or_insert_with(||format!("{}; {}{}: {}",item.source,
                    item.title,item.page.map(|p|format!(", страница {p}")).unwrap_or_default(),item.basis));
            }
        }
    }
    let counts=block1::counted_unique_codes(inputs.iter().copied());
    let operations=block1::work_operations(db,package_id).await?;
    let mut minimums=block1::effective_minimums(&block1::minimums(db,package_id).await?,&operations);
    if project_state.signed {
        for item in &project_state.approved {
            let count=minimums.entry(item.code.clone()).or_insert(1);
            *count=(*count).max(item.count);
        }
    }
    let scope_approved:bool=sqlx::query_scalar("SELECT block1_scope_approved_at IS NOT NULL FROM packages WHERE id=$1")
        .bind(package_id).fetch_one(db).await?;
    if !scope_approved {
        findings.push(FindingInput::new(1,"WORK-SCOPE-PENDING","Реестр фактически выполненных работ и их объёмов не подтверждён инженером. Полную комплектность по ПД/РД установить нельзя."));
    }
    let work_links=block1::work_evidence(db,package_id).await?;
    for op in &operations {
        let linked=block1::work_evidence_count(op,&work_links,&inputs);
        if linked<(op.required_count as usize) {
            let f=FindingInput::new(1,format!("ZONE-{}",op.id),format!("Не подтверждён состав ИД для работы «{}», зона «{}», код {}. Требуется актов: {}, привязано из этой поставки: {}. Основание: {}",op.description,op.location,op.catalog_code,op.required_count,linked,op.basis)).mismatch();
            findings.push(f);
        }
        if !required.contains(&op.catalog_code) {
            findings.push(FindingInput::new(1,format!("WORK-{}",op.id),format!("В реестре работ указано «{}» ({}), но позиция {} не включена в обязательный чек-лист. Основание: {}",op.description,op.location,op.catalog_code,op.basis)).mismatch());
        }
    }
    for code in &required {
        match catalog.item(package.catalog_name.as_deref().unwrap_or(""),code){
            None=>findings.push(FindingInput::new(1,"C-GONE",format!("Пункт {code} отсутствует в каталоге. Проверьте настройки."))),
            Some(item)=>{
                let expected=minimums.get(code).copied().unwrap_or(1).max(1) as usize;
                let present=counts.get(code).copied().unwrap_or(0);
                if present<expected {
                    let mut f=FindingInput::new(1,format!("C-{}",code),format!("Недостаточно документов: {} (пункт {}). Требуется: {}, найдено: {} в данной поставке.",item.title,code,expected,present)).mismatch();
                    f.reference=project_basis.get(code)
                        .cloned().unwrap_or_else(||format!("Каталог ИД: {}",item.norm_reference));
                    if let Some(basis)=project_basis.get(code) {f.ntd=basis.clone();}
                    findings.push(f);
                }
            }
        }
    }
    // Project-specific checklist entries, editable without changing the global catalog.
    let custom_required: Vec<(String,String)> = sqlx::query_as(
        "SELECT code,title FROM package_custom_checklist WHERE package_id=$1 AND active=true AND required=true")
        .bind(package_id).fetch_all(db).await?;
    let custom_count=custom_required.len();
    let mut custom_done=0usize;
    for (code,title) in custom_required {
        if counts.get(&code).copied().unwrap_or(0)==0 {
            findings.push(FindingInput::new(1,format!("C-{code}"),
                format!("Не найден обязательный документ: {title} (добавлен в чек-лист комплекта)." )).mismatch());
        }else{custom_done+=1;}
    }
    if package.catalog_name.is_none(){findings.push(FindingInput::new(1,"C-UNKNOWN","Вид работ не сопоставлен с каталогом. Выберите применимый перечень."));}
    let approval:Option<(String,)> = sqlx::query_as("SELECT block1_approved_by FROM packages WHERE id=$1 AND block1_approved_at IS NOT NULL")
        .bind(package_id).fetch_optional(db).await?;
    if approval.is_none(){
        findings.push(FindingInput::new(1,"C-UNAPPROVED","Обязательный перечень первого блока не утверждён инженером. Настройте состав, количество экземпляров и применимые приложения, затем нажмите «Утвердить перечень»."));
    }
    // Conditional supplier certificates, laboratory protocols and appendices:
    // only engineer-activated requirements are mandatory; never invent applicability.
    let requirements=block1::support_requirements(db,package_id).await?;
    let links=block1::linked_evidence(db,package_id).await?;
    for r in &requirements {
        let parent_ok=block1::requirement_parent_valid(r,&inputs);
        if !parent_ok {
            let problem=if r.parent_document_id.is_none(){"Не указан акт, к которому относится обязательное приложение"}
                else{"Приложение привязано к акту другой поставки"};
            findings.push(FindingInput::new(1,format!("PARENT-{}",r.id),format!("{}: «{}». Укажите актуальный акт из выбранной ревизии.",problem,r.title)).mismatch());
        }
        let present=if parent_ok {
            // A document cannot be its own appendix.
            let eligible:Vec<&Document>=inputs.iter().copied().filter(|d|Some(d.id)!=r.parent_document_id).collect();
            block1::evidence_count(r.id,&links,&eligible)
        }else{0};
        if present<(r.min_count as usize) {
            let kind=match r.kind.as_str(){"supplier"=>"Документ качества поставщика","laboratory"=>"Лабораторное испытание",_=>"Приложение к акту"};
            let mut f=FindingInput::new(1,format!("S-{}",r.id),format!("{}: {}. Требуется подтверждённых файлов: {}, связано с текущей поставкой: {}. Условие применимости: {}.",kind,r.title,r.min_count,present,if r.applies_when.is_empty(){"определено инженером"}else{&r.applies_when})).mismatch();
            f.ntd=r.basis.clone();
            findings.push(f);
        }
    }
    if !block1::complete_metadata(&inputs) && !inputs.is_empty(){
        findings.push(FindingInput::new(1,"METADATA-INCOMPLETE","Не все документы ИД классифицированы и успешно распознаны. Положительный процент не подтверждает полную комплектность."));
    }
    if !inputs.is_empty() && required.is_empty() && custom_count==0 && requirements.is_empty() {
        findings.push(FindingInput::new(1,"C-NOT-CONFIGURED","Для комплекта не задан обязательный состав документов и приложений. Автоматически считать комплект полным нельзя."));
    }
    for doc in &inputs {
        if doc.catalog_code.is_none(){findings.push(FindingInput::new(1,"CLASSIFY","Тип документа не определён достоверно. Требуется ручная классификация.").doc(doc));}
        if !matches!(doc.recognition_status.as_str(), "extracted"|"ocr"|"ocr_vibe"|"ocr_local") {
            findings.push(FindingInput::new(3,"OCR",format!("Текст распознан не полностью: {}. Соответствие не подтверждено.",doc.recognition_status)).doc(doc));
        }
        if doc.recognition_confidence.is_some_and(|c|c<0.7){
            findings.push(FindingInput::new(3,"OCR-CONF","Низкая уверенность распознавания (<70%). Нужна ручная проверка.").doc(doc));
        }
        if doc.catalog_code.is_some() && doc.document_type.as_ref().is_some_and(|s|s.to_lowercase().contains("акт")) && !ANY_DATE.is_match(&doc.recognized_text){
            findings.push(FindingInput::new(3,"E08","Не удалось определить дату акта; проверьте обязательные поля.").doc(doc));
        }
        for message in timeline_discrepancies(&doc.recognized_text){
            let mut f=FindingInput::new(3,"E12",message).doc(doc);f.severity="high".into();findings.push(f);
        }
        for rule in &norm_rules {
            if !rule.document_code.is_empty() && doc.catalog_code.as_deref()!=Some(rule.document_code.as_str()) {continue;}
            matched_rules+=1;
            let (mut status,mut details)=norms::evaluate(rule,&doc.recognized_text,&doc.recognition_status,doc.recognition_confidence);
            // A text-layer PDF can hide image-only pages: do not conclude absence from it.
            let ext=doc.relative_path.rsplit('.').next().unwrap_or("").to_lowercase();
            if status=="mismatch" && ext!="docx" {
                status="manual".into();
                details=format!("Предварительная проверка (неполный контроль структуры {ext}): {details}");
            }
            let basis=norms::basis(rule);
            norm_results.push((doc.id,rule.id,rule.version,status.clone(),details.clone(),basis.clone(),rule.source_reference.clone()));
            if status!="evidence_found" {
                let mut f=FindingInput::new(if rule.check_kind=="manual" {2}else{4},format!("N-{}-v{}",rule.id,rule.version),format!("{}: {}",rule.description,details)).doc(doc);
                f.result=if status=="mismatch" {"mismatch"}else{"manual"}.into();
                f.severity=if status=="mismatch" {"high"}else{"review"}.into();
                f.ntd=basis;
                f.reference=rule.source_reference.clone();
                findings.push(f);
            }
        }

    }
    if !inputs.is_empty(){
        if norm_rules.is_empty(){
            findings.push(FindingInput::new(4,"N-RULES-NONE","Нет утверждённых нормативных правил для вида работ. Комплект нельзя считать проверенным по ГОСТ/СП/НТД."));
        }else if matched_rules==0 {
            findings.push(FindingInput::new(4,"N-NOT-APPLICABLE","Утверждённые нормативные правила не сопоставились с типами загруженных документов; требуется настройка и классификация."));
        }
        if !reference_exists {
            findings.push(FindingInput::new(2,"NO-REFERENCE","Нет эталонных ПД/РД. Сопоставление размеров, отметок, материалов и шифров невозможно."));
        }else{
            findings.push(FindingInput::new(2,"DRAWING-REVIEW","Сопоставление чертежей, допусков и материалов требует проверки инженера (этап 3 ТЗ)."));
        }
        findings.push(FindingInput::new(4,"FORMS-REVIEW","Полное соответствие форм ГОСТ/СП/приказам нельзя подтвердить поиском реквизитов: инженер должен сверить применимый утверждённый шаблон и визуальное оформление."));
    }
    // Freeze the quantitative outcome as of THIS check run. A newer contractor
    // revision or a changed checklist must never rewrite historical results.
    let catalog_done=required.iter().filter(|code|catalog.item(package.catalog_name.as_deref().unwrap_or(""),code).is_some() &&
        counts.get(*code).copied().unwrap_or(0)>=minimums.get(*code).copied().unwrap_or(1).max(1) as usize).count();
    let work_done=operations.iter().filter(|w|block1::work_evidence_count(w,&work_links,&inputs)>=w.required_count as usize).count();
    let support_done=requirements.iter().filter(|r|{
        if !block1::requirement_parent_valid(r,&inputs){return false;}
        let eligible:Vec<&Document>=inputs.iter().copied().filter(|d|Some(d.id)!=r.parent_document_id).collect();
        block1::evidence_count(r.id,&links,&eligible)>=r.min_count as usize
    }).count();
    let total=required.len()+custom_count+operations.len()+requirements.len();
    let confirmed=catalog_done+custom_done+work_done+support_done;
    let checks_ready=approval.is_some() && scope_approved &&
        (project_state.candidates==0 || project_state.signed) && !inputs.is_empty() && total>0 &&
        block1::complete_metadata(&inputs) && delivery.map(|d|block1::manifest_problems(d,&inputs).is_empty()).unwrap_or(!has_delivery_history) &&
        !findings.iter().any(|f|f.block==1 && f.result=="manual" &&
            matches!(f.rule_code.as_str(),"C-UNKNOWN"|"C-NOT-CONFIGURED"|"IMPORT-GAP"|"CLASSIFY"));
    let coverage=if checks_ready {Some(((confirmed*100)/total).min(100) as i32)}else{None};
    let ready_for_engineer_review=coverage==Some(100) && !findings.iter().any(|f|f.block==1 && f.result=="mismatch");
    let project_sig=project_requirements::source_signature(db,package.project_id).await?;
    let project_snapshot=serde_json::json!({
      "source_signature": project_sig,
      "signed": project_state.signed,
      "pending_count": project_state.pending,
      "required": project_state.approved.iter().map(|r|serde_json::json!({
        "candidate_id":r.candidate_id,"catalog_code":r.code,"min_count":r.count,
        "source":r.source,"page":r.page,"excerpt":r.basis
      })).collect::<Vec<_>>()
    });
    let mut tx=db.begin().await?;
    let run_id:(i64,)=sqlx::query_as("INSERT INTO check_runs(package_id,source,rule_version,submission_id) VALUES($1,$2,3,$3) RETURNING id")
        .bind(package_id).bind(source).bind(delivery.map(|d|d.id)).fetch_one(&mut *tx).await?;
    sqlx::query("INSERT INTO check_run_project_requirements(run_id,source_signature,signed,snapshot) VALUES($1,$2,$3,$4)")
        .bind(run_id.0).bind(&project_sig).bind(project_state.signed).bind(project_snapshot)
        .execute(&mut *tx).await?;
    sqlx::query("INSERT INTO check_run_block1_metrics(run_id,required_total,confirmed_total,coverage_pct,ready_for_engineer_review) VALUES($1,$2,$3,$4,$5)")
        .bind(run_id.0).bind(total as i32).bind(confirmed as i32).bind(coverage)
        .bind(ready_for_engineer_review).execute(&mut *tx).await?;
    for (document_id,rule_id,rule_version,result,details,normative_basis,source_reference) in norm_results {
        sqlx::query("INSERT INTO normative_check_results(run_id,package_id,document_id,rule_id,rule_version,result,details,normative_basis,source_reference) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(run_id.0).bind(package_id).bind(document_id).bind(rule_id).bind(rule_version)
            .bind(result).bind(details).bind(normative_basis).bind(source_reference)
            .execute(&mut *tx).await?;
    }
    for f in findings{
        sqlx::query("INSERT INTO findings(run_id,package_id,document_id,block,rule_code,description,source_location,reference,ntd,severity,result) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
            .bind(run_id.0).bind(package_id).bind(f.document_id).bind(f.block).bind(f.rule_code)
            .bind(f.description).bind(f.source_location).bind(f.reference).bind(f.ntd)
            .bind(f.severity).bind(f.result).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(run_id.0)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]fn detects_later_protocol(){
        let t="Акт №15 от 06.09.2026\nПротокол №32 от 09.09.2026";
        assert_eq!(timeline_discrepancies(t).len(),1);
    }
    #[test]fn does_not_invent_discrepancy(){assert!(timeline_discrepancies("Акт №15 от 09.09.2026").is_empty());}
}
