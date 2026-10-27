//! What the bulk tests share. A module of each, not a test of its own.
#![allow(dead_code, reason = "each test uses part of it")]

use std::time::Duration;

use serde_json::{Value, json};
use shopify_api::bulk::BulkWaitOptions;
use shopify_api::transport::HttpRequest;

pub const UPLOAD_URL: &str = "https://shopify-staged-uploads.storage.googleapis.com/";

/// A bulk operation of type `kind` (`QUERY` or `MUTATION`), as Shopify returns it.
pub fn operation_of(kind: &str, status: &str, url: Option<&str>) -> Value {
    json!({
        "id": "gid://shopify/BulkOperation/1",
        "type": kind,
        "status": status,
        "errorCode": null,
        "createdAt": "2026-10-22T10:00:00Z",
        "completedAt": if status == "COMPLETED" { json!("2026-10-22T10:01:00Z") } else { Value::Null },
        "objectCount": "2",
        "rootObjectCount": "2",
        "fileSize": url.map(|_| "120"),
        "url": url,
        "partialDataUrl": null,
        "query": "{ products { edges { node { id title } } } }",
    })
}

/// A bulk query, as Shopify returns it.
pub fn operation(status: &str, url: Option<&str>) -> Value {
    operation_of("QUERY", status, url)
}

pub fn operation_name(request: &HttpRequest) -> Option<String> {
    request
        .json()?
        .get("operationName")?
        .as_str()
        .map(str::to_string)
}

pub fn fast() -> BulkWaitOptions {
    BulkWaitOptions::new().poll_interval(Duration::from_millis(1))
}
