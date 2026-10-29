use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct Item {
    pub code: String,
    pub title: String,
    pub norm_reference: String,
}
#[derive(Clone, Debug, Deserialize)]
pub struct WorkType {
    pub name: String,
    pub items: Vec<Item>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct CommonError {
    pub code: String,
    pub title: String,
    pub description: String,
    pub algorithm: String,
    pub check_type: String,
    pub severity: String,
}
#[derive(Clone)]
pub struct Catalog {
    pub work_types: Vec<WorkType>,
    pub errors: Vec<CommonError>,
}
pub fn normalize(s: &str) -> String { s.to_lowercase().replace('ё', "е") }
/// Conservative document-type label independent of the work checklist.
/// This is ONLY a preliminary UI classification, not confirmation of applicability.
pub fn guess_general_document_type(filename:&str, text:&str)->Option<String>{
    let name=normalize(filename).replace('_'," ").replace('-'," ");
    let intro=normalize(&text.chars().take(1800).collect::<String>());
    let has=|token:&str| name.contains(token) || intro.contains(token);
    let title=if has("реестр исполнительной документации") || (name.contains("реестр") && name.contains("ид")) {
        "Реестр исполнительной документации"
    } else if name.contains("реестр паспорт") {"Реестр паспортов и документов качества"
    } else if has("акт освидетельствования скрытых работ") || name.contains("аоср") {"Акт освидетельствования скрытых работ (АОСР)"
    } else if has("акт освидетельствования ответственных конструкций") {"Акт освидетельствования ответственных конструкций"
    } else if has("исполнительная схема") || has("исполнительные схемы") || name.ends_with(" ис.pdf") || name.ends_with(" ис.png") {
        "Исполнительная схема"
    } else if has("протокол испыт") || name.contains("протокол") {"Протокол испытаний"
    } else if name.contains("приказ") || intro.starts_with("приказ ") {"Приказ о назначении ответственного лица"
    } else if name.contains("паспорт") || intro.starts_with("паспорт ") {"Паспорт / документ качества"
    } else if has("сертификат соответствия") || name.contains("сертификат") {"Сертификат соответствия"
    } else if has("общий журнал работ") || name.contains("ожр") {"Общий журнал работ"
    } else if has("журнал бетонных работ") {"Специальный журнал работ"
    } else {return None};
    Some(title.to_string())
}

impl Catalog {
    pub fn load() -> anyhow::Result<Self> {
        Ok(Self {
            work_types: serde_json::from_str(include_str!("../catalog/work_types.json"))?,
            errors: serde_json::from_str(include_str!("../catalog/common_errors.json"))?,
        })
    }
    pub fn work_type(&self, name: &str) -> Option<&WorkType> {
        self.work_types.iter().find(|t| t.name == name)
    }
    pub fn closest(&self, name: &str) -> Option<&WorkType> {
        self.work_types.iter().max_by(|a,b| {
            strsim::normalized_levenshtein(&normalize(name),&normalize(&a.name))
                .total_cmp(&strsim::normalized_levenshtein(&normalize(name),&normalize(&b.name)))
        }).filter(|t| strsim::normalized_levenshtein(&normalize(name), &normalize(&t.name)) >= 0.85)
    }
    pub fn item(&self, work: &str, code: &str) -> Option<&Item> {
        self.work_type(work)?.items.iter().find(|i| i.code == code)
    }
    /// Conservative semantic routing for a whole *document*, not an assertion of
    /// its engineering correctness. OCR may contain references to other acts,
    /// therefore only the filename and the START of the text are inspected.
    /// Ambiguous documents remain unlinked for an engineer to review.
    pub fn classify(&self, filename: &str, text: &str, work: Option<&WorkType>) -> Option<(String,String)> {
        let work = work?;
        let path = normalize(filename).replace('_', " ").replace('-', " ");
        let header = normalize(&text.chars().take(750).collect::<String>())
            .split_whitespace().collect::<Vec<_>>().join(" ");
        let title_text = format!("{path}\n{header}");
        let has = |needle:&str| title_text.contains(needle);

        // Earthworks has several different kinds of acts. Exact type keywords
        // win over a generic "акт" or incidental references in the appendix.
        // Do not classify a plain "исполнительная схема" as geodetic without
        // additional geodetic / earthwork evidence.
        if work.name=="Земляные работы" {
            let direct: Option<&str> = if has("реестр исполнительной документации") ||
                (path.contains("реестр") && path.contains(" ид")) {Some("1.1")}
            else if has("освидетельствования геодезической разбивочной основы") ||
                has("освидетельствования разбивочной основы") {Some("1.2")}
            else if has("акт разбивки осей") || has("акт о разбивке осей") ||
                has("акт разбивки основных осей") {Some("1.3")}
            else if has("акт освидетельствования скрытых работ") ||
                has("акты освидетельствования скрытых работ") ||
                path.contains("аоср") {Some("1.4")}
            else if has("акт приемки основания") || has("акт приёмки основания") ||
                has("акт приемки грунтового основания") || has("акт приёмки грунтового основания") {Some("1.6")}
            else if (has("исполнительная геодезическая схема") ||
                has("исполнительные геодезические схемы") ||
                (has("исполнительная схема") &&
                 (has("геодезическ") || has("котлован") || has("землян") || has("грунта") || has("планово высотн")))) &&
                !has("акт освидетельствования скрытых работ") {Some("1.5")}
            else if (has("протокол") || has("заключение")) &&
                (has("уплотнения грунта") || has("коэффициента уплотнения") ||
                 has("испытания грунта") || has("плотности грунта")) &&
                (path.contains("протокол") || path.contains("испытан") ||
                 header.starts_with("протокол") || header.starts_with("заключение")) {Some("1.7")}
            else {None};
            if let Some(code)=direct {
                return work.items.iter().find(|i|i.code==code)
                    .map(|i|(i.code.clone(),i.title.clone()));
            }
            // No fuzzy fallback: earthworks' act and scheme codes are easy to
            // confuse. Prefer a visible unclassified file to a false positive.
            return None;
        }

        let aliases = [
            ("реестр исполнительной документации", vec!["реестр исполнительной документации"]),
            ("акты освидетельствования скрытых работ",vec!["освидетельствования скрытых работ", "аоср"]),
            ("акт разбивки осей",vec!["разбивки осей"]),
            ("исполнительные геодезические схемы",vec!["исполнительная геодезическая схема", "исполнительные геодезические схемы"]),
            ("исполнительные схемы",vec!["исполнительная схема", "исполнительные схемы"]),
        ];
        for (title, phrases) in aliases {
            for item in &work.items {
                if normalize(&item.title).contains(title) && phrases.iter().any(|s|has(s)) {
                    return Some((item.code.clone(),item.title.clone()));
                }
            }
        }
        let probe = normalize(filename.rsplit_once('.').map(|(s,_)| s).unwrap_or(filename))
            .replace('_', " ").replace('-', " ");
        let mut ranked: Vec<_> = work.items.iter().map(|i| (strsim::normalized_levenshtein(&probe,&normalize(&i.title)),i)).collect();
        ranked.sort_by(|a,b| b.0.total_cmp(&a.0));
        if let Some((score, item)) = ranked.first() {
            let runner_up = ranked.get(1).map(|(s,_)| *s).unwrap_or(0.0);
            if *score >= 0.88 && *score - runner_up >= 0.08 {
                return Some((item.code.clone(),item.title.clone()));
            }
        }
        None
    }

}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imported_sources() {
        let c=Catalog::load().unwrap();
        assert_eq!(c.work_types.len(), 14);
        assert_eq!(c.work_types.iter().map(|w|w.items.len()).sum::<usize>(),169);
        assert_eq!(c.errors.len(),34);
    }
    #[test]
    fn earthworks_differentiates_seven_document_types(){
        let c=Catalog::load().unwrap();
        let w=c.work_type("Земляные работы").unwrap();
        let tests=[
            ("scan.pdf","Акт освидетельствования геодезической разбивочной основы объекта", "1.2"),
            ("scan.pdf","Акт разбивки осей объекта на местности", "1.3"),
            ("АОСР_07.pdf","", "1.4"),
            ("ИД.pdf","Исполнительная геодезическая схема котлована", "1.5"),
            ("Акт приёмки основания.pdf","", "1.6"),
            ("Протокол_1.pdf","Протокол определения коэффициента уплотнения грунта", "1.7"),
        ];
        for (filename,ocr,code) in tests {
            assert_eq!(c.classify(filename,ocr,Some(w)).unwrap().0,code,"{filename}: {ocr}");
        }
    }
    #[test]
    fn earthworks_does_not_confuse_references_with_document_type(){
        let c=Catalog::load().unwrap();
        let w=c.work_type("Земляные работы").unwrap();
        // A generic cover/index and a generic scheme should not be
        // automatically counted as a specific acceptance/geodetic act.
        assert!(c.classify("scan.pdf","Перечень документов по земляным работам",Some(w)).is_none());
        assert!(c.classify("ИС_12.pdf","Исполнительная схема железобетонной плиты",Some(w)).is_none());
    }
    #[test]
    fn earthworks_and_reestr() {
        let c=Catalog::load().unwrap();
        let w=c.closest("Земляные работы").unwrap();
        assert_eq!(c.classify("Реестр исполнительной документации.xlsx", "", Some(w)).unwrap().0,"1.1");
    }
}
