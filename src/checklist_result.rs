//! Clear, separate quantitative checklist UI for Block 1 (not semantic review).
//! Read-only presentation of the same evidence/count algorithms used by engine.rs.
use crate::{block1::{self, SupportRequirement, WorkOperation},catalog::Catalog,models::Document,project_requirements};
use std::collections::{HashMap,HashSet};

#[derive(Clone)]
pub struct CustomEntry {pub code:String,pub title:String,pub required:bool}

pub struct View<'a> {
    pub catalog: &'a Catalog,
    pub work_type: &'a str,
    pub required: &'a HashSet<String>,
    pub minima: &'a HashMap<String,i32>,
    pub files: &'a [&'a Document],
    pub custom: &'a [CustomEntry],
    pub operations: &'a [WorkOperation],
    pub work_links: &'a HashMap<i64,Vec<i64>>,
    pub support: &'a [SupportRequirement],
    pub support_links: &'a HashMap<i64,Vec<i64>>,
    pub project_approved: &'a [project_requirements::Approved],
    pub has_delivery:bool,
    pub manifest_ok:bool,
    pub checklist_approved:bool,
    pub scope_approved:bool,
    pub project_approval_needed:bool,
    pub historical:bool,
}

#[derive(Debug,PartialEq,Eq,Clone,Copy)]
enum Verdict {Passed,Missing,NeedsReview,NotRequired}
impl Verdict {
    fn caption(self)->&'static str {match self {
        Self::Passed=>"Пройдено",Self::Missing=>"Не подтверждено",Self::NeedsReview=>"Проверить",Self::NotRequired=>"Не применяется",
    }}
    fn class(self)->&'static str {match self {
        Self::Passed=>"passed",Self::Missing=>"missing",Self::NeedsReview=>"review",Self::NotRequired=>"optional",
    }}
}
fn esc(s:&str)->String {html_escape::encode_safe(s).to_string()}

fn ready_for_count(v:&View<'_>,code:&str, found:usize, expected:usize, recognized:bool, unclassified:usize) -> (Verdict,String) {
    if !v.has_delivery {return (Verdict::NeedsReview,"Нет завершённой опубликованной поставки; старые версии ИД не засчитываются".into());}
    if !v.manifest_ok {return (Verdict::NeedsReview,"Опись ZIP или контрольные суммы не подтверждены — результат предварительный".into());}
    if found<expected {
        let gap=expected-found;
        let reason=if unclassified>0 {
            format!("Не хватает подтверждённых документов: {gap}. Есть {unclassified} файлов без типа; сначала назначьте им позицию чек-листа")
        }else{format!("Не хватает подтверждённых документов: {gap} (учтены только файлы выбранной поставки и уникальные SHA-256)")};
        return (Verdict::Missing,reason)
    }
    if !recognized {return (Verdict::NeedsReview,"Количество найдено, но OCR части доказательств не завершён или требует проверки".into());}
    if !v.checklist_approved || !v.scope_approved || v.project_approval_needed {
        let mut issues=vec![];
        if !v.checklist_approved {issues.push("чек-лист не утверждён");}
        if !v.scope_approved {issues.push("реестр работ не подтверждён");}
        if v.project_approval_needed {issues.push("проектные требования ожидают утверждения");}
        return (Verdict::NeedsReview,format!("Документов достаточно по коду {code}, но {}",issues.join(", ")));
    }
    (Verdict::Passed,"Количество подтверждено по классификации и SHA-256; содержательное соответствие проверяется отдельно".into())
}

fn evidence_list<'a>(docs:impl Iterator<Item=&'a Document>)->String {
    let mut result=String::new();let mut seen=HashSet::new();
    for d in docs {
        if !seen.insert(d.sha256.to_lowercase()){continue;}
        let name=d.relative_path.rsplit('/').next().unwrap_or(&d.relative_path);
        result.push_str(&format!("<li><a href='/documents/{}/download' title='Оригинал файла'>{}</a> <small>SHA-256: {}</small></li>",d.id,esc(name),esc(d.sha256.get(..10).unwrap_or(&d.sha256))));
    }
    if result.is_empty(){String::new()}else{format!("<details class='check-evidence'><summary>Засчитанные файлы</summary><ul>{result}</ul></details>")}
}

pub fn render(v:&View<'_>)->String {
    let counts=block1::counted_unique_codes(v.files.iter().copied());
    let unclassified=v.files.iter().filter(|d|d.catalog_code.is_none()).count();
    let mut body=String::from("<section class='completion-panel' id='completion' aria-labelledby='completion-title'><div class='completion-head'><div><h2 id='completion-title'>Блок 1 · Комплектность по чек-листу</h2><p>Для каждого пункта показано: сколько требуется, сколько засчитано из текущей поставки, результат и причина. Это <b>не</b> семантическая экспертиза.</p></div><a class='buttonlink' href='#checklist'>Настроить чек-лист</a></div>");
    if v.historical {body.push_str("<p class='completion-alert'>Открыта историческая проверка. Здесь показан пересчёт выбранной поставки по <b>нынешнему</b> чек-листу, а не сохранённый результат старого запуска. Для старого запуска используйте его замечания и метрики.</p>");}
    if !v.has_delivery {body.push_str("<p class='completion-alert'>Нет опубликованной поставки подрядчика. Отображаемые значения не могут считаться итогом.</p>");}
    if v.project_approval_needed {body.push_str("<p class='completion-alert'>Есть неутверждённые проектные требования. До утверждения инженер не может считать чек-лист окончательным.</p>");}
    let mut passed=0usize;let mut failed=0usize;let mut review=0usize;let mut not_required=0usize;let mut rows=String::new();
    let mut row=|code:&str,title:&str,needed:usize,found:usize,verdict:Verdict,why:String,evidence:String,basis:&str|{
        match verdict{Verdict::Passed=>passed+=1,Verdict::Missing=>failed+=1,Verdict::NeedsReview=>review+=1,Verdict::NotRequired=>not_required+=1};
        rows.push_str(&format!("<tr data-checklist-status='{}'><td><strong>{}</strong><div class='check-item-title'>{}</div><small>{}</small></td><td class='check-num'>{}</td><td class='check-num'>{}</td><td><span class='check-verdict check-{}'>{}</span></td><td><span class='check-reason'>{}</span>{}</td></tr>",
            verdict.class(),esc(code),esc(title),esc(basis),if verdict==Verdict::NotRequired{"—".into()}else{needed.to_string()},found,verdict.class(),verdict.caption(),esc(&why),evidence));
    };
    if let Some(wt)=v.catalog.work_type(v.work_type){
        for item in &wt.items {
            let found=counts.get(&item.code).copied().unwrap_or(0);
            let required=v.required.contains(&item.code);
            let expected=v.minima.get(&item.code).copied().unwrap_or(1).max(1) as usize;
            let proofs:Vec<&Document>=v.files.iter().copied().filter(|d|d.catalog_code.as_deref()==Some(item.code.as_str())).collect();
            let recognized=proofs.iter().all(|d|matches!(d.recognition_status.as_str(),"extracted"|"ocr"|"ocr_vibe"|"ocr_local"));
            let (verdict,reason)=if !required {(Verdict::NotRequired,"Не включено в обязательный перечень. Включите при необходимости и утвердите чек-лист".into())}
               else{ready_for_count(v,&item.code,found,expected,recognized,unclassified)};
            let project_basis=v.project_approved.iter().find(|p|p.code==item.code).map(|p|p.basis.as_str());
            row(&item.code,&item.title,expected,found,verdict,reason,evidence_list(proofs.into_iter()),project_basis.unwrap_or(&item.norm_reference));
        }
    }else{
        body.push_str("<p class='completion-alert'>Вид работ не выбран или отсутствует в каталоге. Настройте вид работ, иначе чек-лист не определён.</p>");
    }
    for c in v.custom {
        // Avoid double counting a custom item already displayed in the catalogue.
        if v.catalog.item(v.work_type,&c.code).is_some(){continue;}
        let found=counts.get(&c.code).copied().unwrap_or(0);
        let expected=v.minima.get(&c.code).copied().unwrap_or(1).max(1) as usize;
        let docs:Vec<&Document>=v.files.iter().copied().filter(|d|d.catalog_code.as_deref()==Some(c.code.as_str())).collect();
        let recognized=docs.iter().all(|d|matches!(d.recognition_status.as_str(),"extracted"|"ocr"|"ocr_vibe"|"ocr_local"));
        let (verdict,reason)=if !c.required {(Verdict::NotRequired,"Дополнительная позиция не отмечена как обязательная".into())}else{ready_for_count(v,&c.code,found,expected,recognized,unclassified)};
        row(&c.code,&c.title,expected,found,verdict,reason,evidence_list(docs.into_iter()),"Дополнительная позиция комплекта");
    }
    // Approved project rules can reference a code that no longer exists in the catalogue.
    for code in v.required {
        if v.catalog.item(v.work_type,code).is_some() || v.custom.iter().any(|c|c.code.as_str()==code.as_str()) {continue;}
        let proposal=v.project_approved.iter().find(|p|p.code.as_str()==code.as_str());
        let title=proposal.map(|p|p.title.as_str()).unwrap_or("Позиция отсутствует в каталоге");
        let basis=proposal.map(|p|p.basis.as_str()).unwrap_or("Необходимо исправить сопоставление");
        let found=counts.get(code).copied().unwrap_or(0);
        let expected=v.minima.get(code).copied().unwrap_or(1).max(1) as usize;
        row(code,title,expected,found,Verdict::NeedsReview,"Код отсутствует в каталоге этого вида работ; инженер должен проверить сопоставление".into(),String::new(),basis);
    }
    for op in v.operations {
        let found=block1::work_evidence_count(op,v.work_links,v.files);
        let expected=op.required_count.max(1) as usize;
        let linked=v.work_links.get(&op.id).map(Vec::as_slice).unwrap_or(&[]);
        let docs:Vec<&Document>=v.files.iter().copied().filter(|d|linked.contains(&d.id) && d.catalog_code.as_deref()==Some(op.catalog_code.as_str())).collect();
        let recognized=docs.iter().all(|d|matches!(d.recognition_status.as_str(),"extracted"|"ocr"|"ocr_vibe"|"ocr_local"));
        let (verdict,reason)=ready_for_count(v,&op.catalog_code,found,expected,recognized,unclassified);
        row(&format!("Зона {}",op.id),&format!("{} — {}",op.description,op.location),expected,found,verdict,reason,evidence_list(docs.into_iter()),&op.basis);
    }
    for req in v.support {
        let parent_ok=block1::requirement_parent_valid(req,v.files);
        let linked=v.support_links.get(&req.id).map(Vec::as_slice).unwrap_or(&[]);
        let docs:Vec<&Document>=v.files.iter().copied().filter(|d|linked.contains(&d.id) && Some(d.id)!=req.parent_document_id).collect();
        let found=if parent_ok {block1::evidence_count(req.id,v.support_links,&docs)}else{0};
        let expected=req.min_count.max(1) as usize;
        let recognized=docs.iter().all(|d|matches!(d.recognition_status.as_str(),"extracted"|"ocr"|"ocr_vibe"|"ocr_local"));
        let (verdict,reason)=if !parent_ok {(Verdict::Missing,"Нет привязки к акту из текущей поставки; документ другой ревизии не засчитывается".into())}
            else {ready_for_count(v,"приложение",found,expected,recognized,unclassified)};
        row(&format!("Прил. {}",req.id),&req.title,expected,found,verdict,reason,evidence_list(docs.into_iter()),&req.basis);
    }
    body.push_str(&format!("<div class='completion-counts'><span class='check-passed'>Пройдено: {passed}</span><span class='check-missing'>Не подтверждено: {failed}</span><span class='check-review'>Проверить: {review}</span><span class='check-optional'>Не применяется: {not_required}</span></div>"));
    body.push_str("<div class='completion-table-scroll'><table class='completion-table'><thead><tr><th>Пункт чек-листа / основание</th><th>Требуется</th><th>Найдено</th><th>Результат</th><th>Почему / доказательства</th></tr></thead><tbody>");
    body.push_str(&rows);
    body.push_str("</tbody></table></div>");
    if unclassified>0 {
        body.push_str(&format!("<details class='completion-unclassified'><summary>Требуют классификации: {unclassified} файлов — это не замечания о комплектности</summary><ul>"));
        for d in v.files.iter().filter(|d|d.catalog_code.is_none()){
            let name=d.relative_path.rsplit('/').next().unwrap_or(&d.relative_path);
            body.push_str(&format!("<li><a href='/documents/{}/download'>{}</a> · <a href='#documents'>Назначить тип</a></li>",d.id,esc(name)));
        }
        body.push_str("</ul></details>");
    }
    body.push_str("<p class='completion-footnote'>«Пройдено» означает количественную комплектность по зарегистрированным и классифицированным файлам текущей поставки; это не подтверждение соответствия проекта, ГОСТ/СП или качества строительных работ. Несопоставленные файлы требуют классификации, но не считаются автоматически отсутствующими.</p>");
    body.push_str("<p class='completion-links'><a href='#documents'>Файлы без классификации и документы →</a> <a href='#remarks'>Замечания инженера →</a> <a href='#semantic-block1'>Отдельная семантическая проверка →</a></p></section>");
    body
}

#[cfg(test)] mod tests {
 use super::*;
 #[test] fn expected_without_delivery_never_passes(){
  let v=View {catalog: &Catalog{work_types:vec![],errors:vec![]},work_type:"",required:&HashSet::new(),minima:&HashMap::new(),files:&[],custom:&[],operations:&[],work_links:&HashMap::new(),support:&[],support_links:&HashMap::new(),project_approved:&[],has_delivery:false,manifest_ok:false,checklist_approved:true,scope_approved:true,project_approval_needed:false,historical:false};
  assert_eq!(ready_for_count(&v,"1.4",1,1,true,0).0,Verdict::NeedsReview);
 }
 #[test] fn shortfall_with_unclassified_is_not_a_definitive_missing_file(){
  let v=View {catalog: &Catalog{work_types:vec![],errors:vec![]},work_type:"",required:&HashSet::new(),minima:&HashMap::new(),files:&[],custom:&[],operations:&[],work_links:&HashMap::new(),support:&[],support_links:&HashMap::new(),project_approved:&[],has_delivery:true,manifest_ok:true,checklist_approved:true,scope_approved:true,project_approval_needed:false,historical:false};
  let (status,reason)=ready_for_count(&v,"1.4",0,2,true,3);
  assert_eq!(status,Verdict::Missing);assert!(reason.contains("3 файлов без типа"));
 }
}
