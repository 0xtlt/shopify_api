//! Bulk operations against a scripted Shopify.
#![cfg(feature = "testing")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use shopify_api::bulk::{BulkOperationStatus, BulkOperationsFilter, BulkWaitOptions};
use shopify_api::testing::{MockTransport, client};
use shopify_api::transport::{HttpRequest, HttpResponse, Method};
use shopify_api::{Error, Shopify};

const RESULTS_URL: &str = "https://storage.googleapis.com/shopify-tiers-assets-prod-us-east1/bulk-operation-outputs/1.jsonl";
const UPLOAD_URL: &str = "https://shopify-staged-uploads.storage.googleapis.com/";

fn operation(status: &str, url: Option<&str>) -> Value {
    json!({
        "id": "gid://shopify/BulkOperation/1",
        "type": "QUERY",
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

fn operation_name(request: &HttpRequest) -> Option<String> {
    request
        .json()?
        .get("operationName")?
        .as_str()
        .map(str::to_string)
}

fn fast() -> BulkWaitOptions {
    BulkWaitOptions::new().poll_interval(Duration::from_millis(1))
}

#[tokio::test]
async fn a_bulk_query_runs_to_its_end_and_its_rows_are_read() {
    let mock = MockTransport::new();
    let polls = Arc::new(Mutex::new(0));
    mock.respond_with(move |request| {
        if request.url == RESULTS_URL {
            assert_eq!(request.method, Method::Get);
            assert_eq!(request.header("X-Shopify-Access-Token"), None, "the token stays with Shopify");
            return Ok(HttpResponse::new(
                200,
                "{\"id\":\"gid://shopify/Product/1\",\"title\":\"Shirt\"}\n{\"id\":\"gid://shopify/Product/2\",\"title\":\"Hat\"}\n",
            ));
        }
        let data = match operation_name(request).as_deref() {
            Some("BulkOperationRunQuery") => {
                json!({ "bulkOperationRunQuery": { "bulkOperation": operation("CREATED", None), "userErrors": [] } })
            }
            Some("BulkOperation") => {
                let mut polls = polls.lock().unwrap();
                *polls += 1;
                let status = if *polls < 3 { "RUNNING" } else { "COMPLETED" };
                let url = (status == "COMPLETED").then_some(RESULTS_URL);
                json!({ "bulkOperation": operation(status, url) })
            }
            other => panic!("unexpected operation {other:?}"),
        };
        Ok(HttpResponse::json(200, &json!({ "data": data })))
    });
    let shopify = client(&mock);

    #[derive(Debug, Deserialize, PartialEq)]
    struct Product {
        id: String,
        title: String,
    }
    let products: Vec<Product> = shopify
        .bulk_query_rows("{ products { edges { node { id title } } } }", fast())
        .await
        .unwrap();
    assert_eq!(
        products
            .iter()
            .map(|product| product.title.as_str())
            .collect::<Vec<_>>(),
        ["Shirt", "Hat"]
    );

    let sent = mock.graphql_requests();
    assert_eq!(
        sent[0].variables,
        json!({ "query": "{ products { edges { node { id title } } } }", "groupObjects": false })
    );
    assert_eq!(sent.len(), 4, "a start and three polls");
}

#[tokio::test]
async fn a_refused_bulk_query_returns_the_user_errors() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "bulkOperationRunQuery": {
        "bulkOperation": null,
        "userErrors": [{ "field": ["query"], "message": "Bulk queries cannot contain a connection argument named first.", "code": "INVALID" }],
    } }));
    let shopify = client(&mock);

    let error = shopify
        .run_bulk_query("{ products(first: 1) { nodes { id } } }")
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "bulkOperationRunQuery refused: query: Bulk queries cannot contain a connection argument named first."
    );
    let Error::UserErrors { errors, .. } = error else {
        panic!("user errors");
    };
    assert_eq!(errors[0].code.as_deref(), Some("INVALID"));
}

#[tokio::test]
async fn a_failed_operation_is_an_error() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "bulkOperationRunQuery": { "bulkOperation": operation("CREATED", None), "userErrors": [] } }));
    let mut failed = operation("FAILED", None);
    failed["errorCode"] = json!("ACCESS_DENIED");
    failed["partialDataUrl"] = json!(RESULTS_URL);
    mock.push_data(json!({ "bulkOperation": failed }));
    let shopify = client(&mock);

    let error = shopify
        .bulk_query_rows::<Value>("{ orders { edges { node { id } } } }", fast())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "bulk operation: the bulk operation gid://shopify/BulkOperation/1 ended Failed (ACCESS_DENIED)"
    );
    // The operation stays reachable: what failed, and what was written before.
    let Error::Bulk {
        operation: Some(operation),
        ..
    } = error
    else {
        panic!("{error:?}");
    };
    assert_eq!(operation.status, BulkOperationStatus::Failed);
    assert_eq!(operation.error_code.as_deref(), Some("ACCESS_DENIED"));
    assert_eq!(operation.partial_data_url.as_deref(), Some(RESULTS_URL));
}

#[tokio::test]
async fn a_bulk_mutation_uploads_its_variables_then_runs() {
    let mock = MockTransport::new();
    let uploaded = Arc::new(Mutex::new(None));
    let seen = uploaded.clone();
    mock.respond_with(move |request| {
        if request.url == UPLOAD_URL {
            *seen.lock().unwrap() = Some(request.clone());
            return Ok(HttpResponse::new(201, ""));
        }
        let data = match operation_name(request).as_deref() {
            Some("StagedUploadsCreate") => json!({ "stagedUploadsCreate": {
                "stagedTargets": [{
                    "url": UPLOAD_URL,
                    "resourceUrl": null,
                    "parameters": [
                        { "name": "key", "value": "tmp/1/bulk/vars.jsonl" },
                        { "name": "policy", "value": "abc" },
                    ],
                }],
                "userErrors": [],
            } }),
            Some("BulkOperationRunMutation") => {
                let mut started = operation("CREATED", None);
                started["type"] = json!("MUTATION");
                json!({ "bulkOperationRunMutation": { "bulkOperation": started, "userErrors": [] } })
            }
            other => panic!("unexpected operation {other:?}"),
        };
        Ok(HttpResponse::json(200, &json!({ "data": data })))
    });
    let shopify = client(&mock);

    let rows = [
        json!({ "product": { "id": "gid://shopify/Product/1", "title": "A" } }),
        json!({ "product": { "id": "gid://shopify/Product/2", "title": "B" } }),
    ];
    let mutation = "mutation($product: ProductUpdateInput!) { productUpdate(product: $product) { userErrors { message } } }";
    let started = shopify.run_bulk_mutation(mutation, &rows).await.unwrap();
    assert_eq!(started.kind, shopify_api::bulk::BulkOperationType::Mutation);
    assert_eq!(started.status, BulkOperationStatus::Created);

    let sent = mock.graphql_requests();
    assert_eq!(
        sent[0].variables,
        json!({ "input": [{ "filename": "bulk_op_vars.jsonl", "mimeType": "text/jsonl", "resource": "BULK_MUTATION_VARIABLES", "httpMethod": "POST" }] })
    );
    assert_eq!(
        sent[1].variables,
        json!({ "mutation": mutation, "stagedUploadPath": "tmp/1/bulk/vars.jsonl" })
    );

    let upload = uploaded.lock().unwrap().clone().expect("an upload");
    let body = String::from_utf8(upload.body.clone()).unwrap();
    assert!(
        upload
            .header("content-type")
            .unwrap()
            .starts_with("multipart/form-data; boundary=")
    );
    assert!(body.contains("name=\"policy\"\r\n\r\nabc\r\n"), "{body}");
    assert!(
        body.contains(
            "{\"product\":{\"id\":\"gid://shopify/Product/1\",\"title\":\"A\"}}\n{\"product\":{\"id\":\"gid://shopify/Product/2\",\"title\":\"B\"}}\n"
        ),
        "{body}"
    );
}

#[tokio::test]
async fn bulk_operations_are_listed_and_cancelled() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "bulkOperations": {
        "nodes": [operation("RUNNING", None)],
        "pageInfo": { "hasNextPage": true, "endCursor": "c1" },
    } }));
    let mut canceling = operation("CANCELING", None);
    canceling["objectCount"] = json!("12");
    mock.push_data(
        json!({ "bulkOperationCancel": { "bulkOperation": canceling, "userErrors": [] } }),
    );
    let shopify = client(&mock);

    let page = shopify
        .bulk_operations(BulkOperationsFilter::new().first(5).query("status:RUNNING"))
        .await
        .unwrap();
    assert_eq!(page.items[0].status, BulkOperationStatus::Running);
    assert_eq!(page.end_cursor.as_deref(), Some("c1"));
    assert_eq!(
        mock.graphql_requests()[0].variables,
        json!({ "first": 5, "query": "status:RUNNING", "reverse": true })
    );

    let cancelled = shopify
        .cancel_bulk_operation("gid://shopify/BulkOperation/1")
        .await
        .unwrap();
    assert_eq!(cancelled.status, BulkOperationStatus::Canceling);
    assert_eq!(cancelled.object_count, 12);
}

#[tokio::test(start_paused = true)]
async fn waiting_gives_up_after_its_timeout() {
    let mock = MockTransport::new();
    mock.respond_with(|_| {
        Ok(HttpResponse::json(
            200,
            &json!({ "data": { "bulkOperation": operation("RUNNING", None) } }),
        ))
    });
    let shopify: Shopify = client(&mock);

    let error = shopify
        .wait_for_bulk_operation(
            "gid://shopify/BulkOperation/1",
            BulkWaitOptions::new().timeout(Duration::from_secs(30)),
        )
        .await
        .unwrap_err();
    // The ID, to cancel the operation that keeps running.
    let Error::Timeout { id, .. } = &error else {
        panic!("{error:?}");
    };
    assert_eq!(id.as_deref(), Some("gid://shopify/BulkOperation/1"));
    // 1 + 2 + 4 + 8 + 10 seconds, then the 5 that are left.
    assert_eq!(mock.requests().len(), 6);
}

/// Counts the warnings of the current thread.
struct Warnings(Arc<Mutex<Vec<String>>>);

impl tracing::Subscriber for Warnings {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        *metadata.level() <= tracing::Level::WARN
    }
    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        self.0
            .lock()
            .unwrap()
            .push(event.metadata().name().to_string());
    }
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}
}

/// The crate's operations go to a pinned version, without a warning that
/// would tell the app to change its own schema.
#[tokio::test]
async fn the_operations_of_the_crate_follow_a_pinned_version_unwarned() {
    let warnings = Arc::new(Mutex::new(Vec::new()));
    let _guard = tracing::subscriber::set_default(Warnings(warnings.clone()));
    let mock = MockTransport::new();
    mock.push_data(json!({ "bulkOperation": null }));
    let shopify = client(&mock).with_api_version(shopify_api::ApiVersion::V2026_07);

    assert!(
        shopify
            .bulk_operation("gid://shopify/BulkOperation/1")
            .await
            .unwrap()
            .is_none()
    );
    assert!(mock.requests()[0].url.contains("/admin/api/2026-07/"));
    assert!(warnings.lock().unwrap().is_empty(), "{warnings:?}");
}
