//! UI release identifiers generated from actual UI source assets at build time.
//! No database migration, environment setting or reverse-proxy change is needed.
use axum::{http::header, response::{IntoResponse, Response}, Json};
use serde_json::json;

pub fn html_id() -> &'static str { env!("IDCONTROL_UI_HTML_ID") }
pub fn js_id() -> &'static str { env!("IDCONTROL_UI_JS_ID") }
pub fn css_id() -> &'static str { env!("IDCONTROL_UI_CSS_ID") }
pub fn images_id() -> &'static str { env!("IDCONTROL_UI_IMAGES_ID") }

pub fn meta_tags() -> String {
    format!(r#"<meta name="idcontrol-html-id" content="{}"><meta name="idcontrol-js-id" content="{}"><meta name="idcontrol-css-id" content="{}"><meta name="idcontrol-images-id" content="{}">"#,
        html_id(), js_id(), css_id(), images_id())
}
pub fn watcher_tag() -> String {
    format!(r#"<script defer src="/assets/ui-version-watch.js?v={}"></script>"#, js_id())
}

pub async fn version() -> Response {
    ([(header::CACHE_CONTROL, "no-store, no-cache, must-revalidate, max-age=0"),
      (header::PRAGMA, "no-cache")],
    Json(json!({"html":html_id(),"js":js_id(),"css":css_id(),"images":images_id()}))).into_response()
}
pub async fn watcher_js() -> Response {
    ([(header::CONTENT_TYPE, "application/javascript; charset=utf-8"),
      (header::CACHE_CONTROL, "no-store, no-cache, must-revalidate, max-age=0")],
      include_str!("../assets/ui-version-watch.js")).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_ui_ids_are_separate() {
        assert!(html_id().starts_with("html-"));
        assert!(js_id().starts_with("js-"));
        assert!(css_id().starts_with("css-"));
        assert!(images_id().starts_with("images-"));
        assert!(meta_tags().contains(html_id()));
        assert!(watcher_tag().contains(js_id()));
    }
}
