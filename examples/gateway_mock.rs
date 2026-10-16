//! Local test server for the privately specified Cowork gateway contract.
//! NEVER connects to GigaCowork, NEVER sends data outside localhost.
use axum::{routing::post, Json, Router};
use serde_json::{json,Value};
use tokio::net::TcpListener;

async fn review(Json(payload):Json<Value>)->Json<Value>{
    let count=payload.get("documents").and_then(|v|v.as_array()).map_or(0,|d|d.len());
    println!("Mock gateway received {count} document excerpts; returns no findings");
    Json(json!({"suggestions":[]}))
}
#[tokio::main]
async fn main()->anyhow::Result<()> {
    let server=Router::new().route("/v1/document-review",post(review));
    let listener=TcpListener::bind("127.0.0.1:9011").await?;
    println!("TEST ONLY: http://127.0.0.1:9011/v1/document-review");
    axum::serve(listener,server).await?;
    Ok(())
}
