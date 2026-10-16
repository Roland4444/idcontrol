//! Vision OCR via Bitrix VibeCode, compatible with the user's verified Rust test.
//! Original image bytes are passed without transcoding. PDF pages MUST be rendered
//! to individual images separately because PDF data: URLs are not supported.
use anyhow::{anyhow, bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct VibeOcr {
    client: Client,
    pub endpoint: String,
    pub key: String,
    pub model: String,
    pub max_tokens: u32,
    pub timeout_seconds:u64,
}

#[derive(Debug, Clone)]
pub struct TextResult {
    pub text: String,
    pub truncated: bool,
}

const OCR_INSTRUCTION: &str = "Извлеки ВЕСЬ видимый текст из изображения документа максимально дословно, в естественном порядке чтения. Сохраняй кириллицу, латиницу, номера, даты, обозначения ГОСТ/СП, марки и классы материалов, размеры, отметки, подписи и таблицы. Для таблиц сохраняй строки, колонки разделяй символом |. Не преобразовывай в JSON, не пересказывай, не исправляй ошибки и не выдумывай текст. Нечитаемое обозначай [НЕРАЗБОРЧИВО]. Ответь только распознанным текстом.";

/// Unlike a generic MIME-driven vision client, the user's working VibeCode
/// example sends raw TIFF/BMP/GIF bytes under a data:image/jpeg URI and the
/// server detects the real image format. Keep that compatibility behavior.
/// With png/jpeg use their actual MIME. This is intentionally NOT transcoding.
fn data_uri_mime(actual_mime: &str) -> Result<&'static str> {
    match actual_mime {
        "image/jpeg" => Ok("image/jpeg"),
        "image/png" => Ok("image/png"),
        "image/tiff" | "image/bmp" | "image/gif" | "image/webp" => Ok("image/jpeg"),
        other => bail!("Unsupported image type for VibeCode: {other}"),
    }
}

/// Provider's successful response has choices[0].message.content as a STRING,
/// not as a JSON table. Detect errors and alternate content forms explicitly.
fn decode_response(value: &Value) -> Result<TextResult> {
    if let Some(err) = value.get("error") {
        let detail = err.get("message").and_then(Value::as_str)
            .or_else(||err.as_str()).unwrap_or("unspecified provider error");
        bail!("VibeCode error: {}", detail.chars().take(240).collect::<String>());
    }
    let choice = value.get("choices").and_then(Value::as_array)
        .and_then(|items|items.first())
        .ok_or_else(||anyhow!("VibeCode response has no choices; JSON keys: {:?}",
            value.as_object().map(|o|o.keys().collect::<Vec<_>>()).unwrap_or_default()))?;
    let content = choice.pointer("/message/content")
        .ok_or_else(||anyhow!("VibeCode response has no choices[0].message.content"))?;
    let text = match content {
        Value::String(s) => s.to_owned(),
        Value::Array(chunks) => chunks.iter()
            .filter_map(|piece| piece.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>().join("\n"),
        Value::Null => bail!("VibeCode returned null content; finish_reason={:?}",choice.get("finish_reason")),
        _ => bail!("VibeCode returned non-text content in message.content"),
    };
    if text.trim().is_empty() {bail!("VibeCode OCR returned empty text");}
    let truncated = choice.get("finish_reason").and_then(Value::as_str) == Some("length");
    Ok(TextResult {text:text.trim().to_owned(),truncated})
}

impl VibeOcr {
    pub fn new(endpoint:&str,key:&str,model:&str,timeout:u64,max_tokens:u32)->Result<Self> {
        let url=reqwest::Url::parse(endpoint).context("Invalid VIBE_OCR_URL")?;
        if url.scheme() != "https" {bail!("VIBE_OCR_URL must use HTTPS");}
        if url.username() != "" || url.password().is_some() || url.fragment().is_some() {
            bail!("VIBE_OCR_URL must not contain credentials or fragments");
        }
        if key.trim().is_empty() {bail!("VIBE_OCR_KEY is empty");}
        let client=Client::builder().timeout(Duration::from_secs(timeout.clamp(10,300))).build()?;
        Ok(Self{client,endpoint:endpoint.to_owned(),key:key.to_owned(),model:model.to_owned(),max_tokens,timeout_seconds:timeout.clamp(10,300)})
    }

    /// Pass the unmodified bytes from JPEG/PNG/TIFF/BMP/GIF/WebP directly,
    /// reproducing the data URI format from the working VibeCode test.
    /// No PDF may be passed here: a separate PDF renderer handles pages.
    pub async fn recognize_image(&self,image:&[u8],source_mime:&str)->Result<TextResult>{
        if image.is_empty() {bail!("Empty image");}
        if image.len() > 20*1024*1024 {bail!("Image larger than 20 MiB; reduce size before OCR");}
        let declared_mime=data_uri_mime(source_mime)?;
        let data_url=format!("data:{declared_mime};base64,{}",STANDARD.encode(image));
        // Start with EXACT request structure from the user's working example.
        let mut payload=json!({"messages":[
            {"role":"system","content":"Ты — модуль точного OCR строительной документации. Не делай нормативных выводов."},
            {"role":"user","content":[
                {"type":"text","text":OCR_INSTRUCTION},
                {"type":"image_url","image_url":{"url":data_url}}
            ]}
        ]});
        // The working example omits both fields; don't force "model": "auto".
        if !self.model.trim().is_empty() && self.model != "auto" {
            payload["model"]=Value::String(self.model.clone());
        }
        if self.max_tokens > 0 {payload["max_tokens"]=json!(self.max_tokens);}

        for attempt in 0..3 {
            let response=self.client.post(&self.endpoint)
                .header("X-Api-Key",&self.key)
                .header("Content-Type","application/json")
                .json(&payload).send().await
                .context("VibeCode OCR HTTP request failed")?;
            let status=response.status();
            if (status==StatusCode::TOO_MANY_REQUESTS || status.is_server_error()) && attempt<2 {
                tracing::warn!(attempt=attempt+1,http_status=%status,"VibeCode временная ошибка; повтор запроса");
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt+1))).await;
                continue;
            }
            if !status.is_success() {
                // Do not print secrets or input image contents to logs.
                let body=response.text().await.unwrap_or_default();
                let msg=serde_json::from_str::<Value>(&body).ok()
                    .and_then(|v|v.pointer("/error/message").and_then(Value::as_str).map(str::to_owned));
                bail!("VibeCode OCR HTTP {status}: {}",msg.unwrap_or("provider request failed".into()));
            }
            let rejected=response.headers().get("x-image-parts-rejected")
                .and_then(|x|x.to_str().ok()).and_then(|x|x.parse::<usize>().ok()).unwrap_or(0);
            let parsed:Value=response.json().await.context("Invalid VibeCode response JSON")?;
            if rejected > 0 || parsed.get("warnings").and_then(Value::as_array)
                .is_some_and(|w| w.iter().any(|x|x.get("code").and_then(Value::as_str)==Some("IMAGE_CONTENT_REJECTED"))) {
                bail!("VibeCode rejected the image part. Check image format and size");
            }
            return decode_response(&parsed);
        }
        bail!("VibeCode OCR retries exhausted")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn recognize_plain_text_from_user_test() {
        let response=json!({"choices":[{"message":{"content":"1) Кабель силовой - 1000 м. $$ 2) Дюбель-хомут - 2000 шт."},"finish_reason":"stop"}]});
        let r=decode_response(&response).unwrap();
        assert!(r.text.contains("Кабель силовой"));
        assert!(!r.truncated);
    }
    #[test] fn missing_content_yields_readable_diagnostic(){
        let response=json!({"error":{"message":"Unsupported PDF input"}});
        assert!(decode_response(&response).unwrap_err().to_string().contains("Unsupported PDF input"));
    }
    #[test] fn recognizes_text_chunks(){
        let response=json!({"choices":[{"message":{"content":[{"type":"text","text":"Первая строка"},{"type":"text","text":"Вторая строка"}]}}]});
        assert_eq!(decode_response(&response).unwrap().text,"Первая строка\nВторая строка");
    }
    #[test] fn user_proven_mime_compatibility() {
        assert_eq!(data_uri_mime("image/tiff").unwrap(),"image/jpeg");
        assert_eq!(data_uri_mime("image/bmp").unwrap(),"image/jpeg");
        assert_eq!(data_uri_mime("image/gif").unwrap(),"image/jpeg");
        assert_eq!(data_uri_mime("image/png").unwrap(),"image/png");
        assert!(data_uri_mime("application/pdf").is_err());
    }
    #[test] fn url_restrictions(){
        assert!(VibeOcr::new("http://example.org/a", "key", "",20,0).is_err());
        assert!(VibeOcr::new("https://vibecode.bitrix24.tech/v1/ai/chat/completions", "key", "",20,0).is_ok());
    }
}
