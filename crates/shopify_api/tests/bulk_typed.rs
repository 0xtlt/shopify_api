//! Bulk operations declared with `graphql!(bulk(…))`, against a scripted Shopify.
#![cfg(feature = "testing")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use shopify_api::bulk::{BulkMutation, BulkMutationResult, BulkWaitOptions};
use shopify_api::testing::{MockTransport, client};
use shopify_api::transport::{HttpRequest, HttpResponse};
use shopify_api::{Error, InvalidValueReason, graphql};

const RESULTS_URL: &str = "https://storage.googleapis.com/shopify-tiers-assets-prod-us-east1/bulk-operation-outputs/1.jsonl";
const UPLOAD_URL: &str = "https://shopify-staged-uploads.storage.googleapis.com/";

graphql! {
    /// Products, their variants with their metafields, and their images.
    pub mod catalog = bulk("{
        products(query: \"status:active\") { edges { node {
            id title
            variants { edges { node { id sku metafields { edges { node { key value } } } } } }
            media { edges { node { ... on MediaImage { id alt } } } }
        } } }
    }");

    mod by_title = bulk("query($filter: String, $sort: ProductSortKeys = TITLE) {
        products(query: $filter, sortKey: $sort, reverse: true) { edges { node { id } } }
    }");

    mod add_tags = bulk("mutation($id: ID!, $tags: [String!]!) {
        tagsAdd(id: $id, tags: $tags) { userErrors { field message } }
    }");
}

fn operation(kind: &str, status: &str, url: Option<&str>) -> Value {
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
        "query": "{ products { edges { node { id } } } }",
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

/// A Shopify that runs any bulk operation at once, its results `jsonl`;
/// the uploads it receives go to `uploads`.
fn shopify_with(jsonl: &'static str) -> (MockTransport, Arc<Mutex<Vec<HttpRequest>>>) {
    let mock = MockTransport::new();
    let uploads = Arc::new(Mutex::new(Vec::new()));
    let seen = uploads.clone();
    mock.respond_with(move |request| {
        if request.url == RESULTS_URL {
            return Ok(HttpResponse::new(200, jsonl));
        }
        if request.url == UPLOAD_URL {
            seen.lock().unwrap().push(request.clone());
            return Ok(HttpResponse::new(201, ""));
        }
        let data = match operation_name(request).as_deref() {
            Some("BulkOperationRunQuery") => json!({ "bulkOperationRunQuery": {
                "bulkOperation": operation("QUERY", "CREATED", None), "userErrors": [],
            } }),
            Some("BulkOperationRunMutation") => json!({ "bulkOperationRunMutation": {
                "bulkOperation": operation("MUTATION", "CREATED", None), "userErrors": [],
            } }),
            Some("StagedUploadsCreate") => json!({ "stagedUploadsCreate": {
                "stagedTargets": [{
                    "url": UPLOAD_URL,
                    "resourceUrl": null,
                    "parameters": [{ "name": "key", "value": "tmp/1/bulk/vars.jsonl" }],
                }],
                "userErrors": [],
            } }),
            Some("BulkOperation") => {
                json!({ "bulkOperation": operation("QUERY", "COMPLETED", Some(RESULTS_URL)) })
            }
            other => panic!("unexpected operation {other:?}"),
        };
        Ok(HttpResponse::json(200, &json!({ "data": data })))
    });
    (mock, uploads)
}

const CATALOG: &str = r#"{"id":"gid://shopify/Product/1","title":"Shirt"}
{"_shopifyApiTypename":"ProductVariant","id":"gid://shopify/ProductVariant/1","sku":"S-1","__parentId":"gid://shopify/Product/1"}
{"key":"color","value":"red","__parentId":"gid://shopify/ProductVariant/1"}
{"_shopifyApiTypename":"MediaImage","id":"gid://shopify/MediaImage/1","alt":"Front","__parentId":"gid://shopify/Product/1"}
{"key":"size","value":"M","__parentId":"gid://shopify/ProductVariant/1"}
{"_shopifyApiTypename":"ProductVariant","id":"gid://shopify/ProductVariant/2","sku":null,"__parentId":"gid://shopify/Product/1"}
{"_shopifyApiTypename":"Video","__parentId":"gid://shopify/Product/1"}
{"id":"gid://shopify/Product/2","title":"Hat"}
"#;

#[tokio::test]
async fn a_bulk_query_sends_its_text_with_group_objects_and_reads_its_items() {
    let (mock, _) = shopify_with(CATALOG);
    let shopify = client(&mock);

    let products: Vec<catalog::Item> = shopify
        .bulk_query(catalog::operation(), fast())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();

    let start = &mock.graphql_requests()[0];
    // `__typename`, aliased, on the nodes of the two connections of a product, which
    // tells their lines apart; not on those of `metafields`, alone under
    // its variant.
    assert_eq!(
        start.variables,
        json!({
            "query": "{ products(query: \"status:active\") { edges { node { id title \
                      variants { edges { node { _shopifyApiTypename: __typename id sku metafields { edges { node { key value } } } } } } \
                      media { edges { node { _shopifyApiTypename: __typename ... on MediaImage { id alt } } } } } } } }",
            "groupObjects": true,
        })
    );
    // The version the query was checked against.
    assert!(mock.requests()[0].url.contains("/admin/api/2026-10/"));

    assert_eq!(products.len(), 2);
    let shirt = &products[0];
    assert_eq!(shirt.title, "Shirt");
    let skus: Vec<Option<&str>> = shirt
        .variants
        .iter()
        .map(|variant| variant.sku.as_deref())
        .collect();
    assert_eq!(skus, [Some("S-1"), None]);
    // Two levels deep: under the variant they belong to.
    let keys: Vec<&str> = shirt.variants[0]
        .metafields
        .iter()
        .map(|field| field.key.as_str())
        .collect();
    assert_eq!(keys, ["color", "size"]);
    assert!(shirt.variants[1].metafields.is_empty());
    // An interface: the fields of a fragment are there when it applies.
    assert_eq!(shirt.media.len(), 2);
    assert_eq!(shirt.media[0].alt.as_deref(), Some("Front"));
    assert_eq!(shirt.media[1].id, None);
    let hat = &products[1];
    assert_eq!(hat.title, "Hat");
    assert!(hat.variants.is_empty() && hat.media.is_empty());
}

#[tokio::test]
async fn the_variables_of_a_bulk_query_are_written_into_its_text() {
    let (mock, _) = shopify_with("");
    let shopify = client(&mock);

    // A string with quotes, a backslash, a newline and a `$`; the default
    // of `$sort`.
    let filter = "title:\"Café \\ $filter\"\nvendor:x";
    let items = shopify
        .bulk_query(
            by_title::Variables {
                filter: Some(filter.to_string()),
                sort: None,
            },
            fast(),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert!(items.is_empty());
    let text = mock.graphql_requests()[0].variables["query"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        text,
        r#"{ products(query: "title:\"Café \\ $filter\"\nvendor:x", sortKey: TITLE, reverse: true) { edges { node { id } } } }"#
    );

    // An enum value bare; a variable without a value or a default left out.
    let query = by_title::operation(by_title::Variables {
        filter: None,
        sort: Some("CREATED_AT".into()),
    });
    assert_eq!(
        query.text().unwrap(),
        "{ products(sortKey: CREATED_AT, reverse: true) { edges { node { id } } } }"
    );
    assert_eq!(query.query(), by_title::QUERY);
}

#[tokio::test]
async fn an_inline_bulk_query_writes_lists_of_enum_values_bare() {
    let (mock, _) = shopify_with(
        "{\"id\":\"gid://shopify/WebhookSubscription/1\",\"topic\":\"ORDERS_CREATE\"}\n",
    );
    let shopify = client(&mock);

    let hooks = shopify
        .bulk_query(
            graphql!(
                bulk("query Hooks($topics: [WebhookSubscriptionTopic!], $format: WebhookSubscriptionFormat, $uri: String) {
                    webhookSubscriptions(topics: $topics, format: $format, uri: $uri, query: \"a $b\") { edges { node { id topic } } }
                }"),
                topics = [ORDERS_CREATE, raw("PRODUCTS_UPDATE")],
                format = JSON,
            ),
            fast(),
        )
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(hooks[0].topic, "ORDERS_CREATE");
    assert_eq!(
        mock.graphql_requests()[0].variables["query"],
        "query Hooks { webhookSubscriptions(topics: [ORDERS_CREATE, PRODUCTS_UPDATE], format: JSON, query: \"a $b\") { edges { node { id topic } } } }"
    );
}

#[tokio::test]
async fn an_enum_value_that_is_not_a_name_fails_before_anything_is_sent() {
    let (mock, _) = shopify_with("");
    let shopify = client(&mock);

    let error = shopify
        .bulk_query(
            graphql!(
                bulk(
                    "query($topics: [WebhookSubscriptionTopic!]) {
                    webhookSubscriptions(topics: $topics) { edges { node { id } } }
                }"
                ),
                topics = [ORDERS_CREATE, raw("X) { shop { name } } #")],
            ),
            fast(),
        )
        .await
        .unwrap_err();
    let Error::InvalidValue { path, reason, .. } = &error else {
        panic!("{error:?}");
    };
    assert_eq!(path, "$topics[1]");
    assert_eq!(*reason, InvalidValueReason::NotAGraphqlName);
    assert!(mock.requests().is_empty());
}

#[tokio::test]
async fn a_child_out_of_place_ends_the_stream() {
    let (mock, _) = shopify_with(
        r#"{"id":"gid://shopify/Product/1","title":"Shirt"}
{"_shopifyApiTypename":"ProductVariant","id":"gid://shopify/ProductVariant/1","sku":"S-1","__parentId":"gid://shopify/Product/9"}
{"id":"gid://shopify/Product/2","title":"Hat"}
"#,
    );
    let shopify = client(&mock);
    let mut items = shopify
        .bulk_query(catalog::operation(), fast())
        .await
        .unwrap();
    let error = items.next_item().await.unwrap().unwrap_err();
    assert_eq!(
        error.to_string(),
        "bulk operation: line 2 of the results is a child of neither the top-level object before it nor one of its children, though the query ran with `groupObjects`"
    );
    assert!(items.next_item().await.is_none());
}

#[tokio::test]
async fn a_child_no_connection_returns_ends_the_stream() {
    for (line, message) in [
        (
            r#"{"_shopifyApiTypename":"Collection","id":"gid://shopify/Collection/1","__parentId":"gid://shopify/Product/1"}"#,
            "line 2 of the results has a `_shopifyApiTypename`, its `__typename`, that no connection of its parent returns",
        ),
        (
            r#"{"id":"gid://shopify/ProductVariant/1","__parentId":"gid://shopify/Product/1"}"#,
            "line 2 of the results has no `_shopifyApiTypename`, the `__typename` that tells which connection of its parent it belongs to",
        ),
    ] {
        let jsonl: &'static str = Box::leak(
            format!("{{\"id\":\"gid://shopify/Product/1\",\"title\":\"Shirt\"}}\n{line}\n")
                .into_boxed_str(),
        );
        let (mock, _) = shopify_with(jsonl);
        let shopify = client(&mock);
        let mut items = shopify
            .bulk_query(catalog::operation(), fast())
            .await
            .unwrap();
        let error = items.next_item().await.unwrap().unwrap_err();
        assert_eq!(error.to_string(), format!("bulk operation: {message}"));
        assert!(items.next_item().await.is_none());
    }
}

#[tokio::test]
async fn a_line_that_does_not_read_as_its_type_ends_the_stream() {
    let (mock, _) = shopify_with(
        r#"{"id":"gid://shopify/Product/1","title":"Shirt"}
{"_shopifyApiTypename":"ProductVariant","id":"gid://shopify/ProductVariant/1","sku":12,"__parentId":"gid://shopify/Product/1"}
{"id":"gid://shopify/Product/2","title":"Hat"}
"#,
    );
    let shopify = client(&mock);
    let mut items = shopify
        .bulk_query(catalog::operation(), fast())
        .await
        .unwrap();
    let error = items.next_item().await.unwrap().unwrap_err();
    assert!(matches!(error, Error::Decode { .. }), "{error:?}");
    assert!(error.to_string().contains("at line 2"), "{error}");
    assert!(items.next_item().await.is_none());
}

#[tokio::test]
async fn the_items_of_an_operation_started_by_hand_are_read_from_its_url() {
    let (mock, _) = shopify_with(CATALOG);
    let shopify = client(&mock);

    let started = shopify
        .start_bulk_query(catalog::operation())
        .await
        .unwrap();
    let operation = shopify
        .wait_for_bulk_operation(&started.id, fast())
        .await
        .unwrap();
    let products: Vec<catalog::Item> = shopify
        .download_bulk_items(operation.url.as_deref().unwrap())
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(products[0].variants[0].metafields.len(), 2);
    assert_eq!(mock.graphql_requests()[0].variables["groupObjects"], true);
}

#[tokio::test]
async fn a_bulk_mutation_stages_its_lines_and_reads_typed_results() {
    let (mock, uploads) = shopify_with(
        r#"{"data":{"tagsAdd":{"userErrors":[]}},"__lineNumber":1}
{"data":{"tagsAdd":{"userErrors":[{"field":["id"],"message":"Product does not exist"}]}},"__lineNumber":0}
{"errors":[{"message":"Variable $id of type ID! was provided invalid value","locations":[{"line":1,"column":10}]}]}
"#,
    );
    let shopify = client(&mock);

    let lines =
        ["gid://shopify/Product/1", "gid://shopify/Product/2"].map(|id| add_tags::Variables {
            id: id.to_string(),
            tags: vec!["sale".to_string(), "a \"b\"".to_string()],
        });
    let mut rows = shopify.bulk_mutation(lines, fast()).await.unwrap();
    let mut results: Vec<BulkMutationResult<add_tags::Data>> = Vec::new();
    while let Some(row) = rows.next_row().await {
        results.push(row.unwrap());
    }

    let sent = mock.graphql_requests();
    assert_eq!(sent[1].variables["mutation"], add_tags::QUERY);
    assert_eq!(
        sent[1].variables["stagedUploadPath"],
        "tmp/1/bulk/vars.jsonl"
    );
    let body = String::from_utf8(uploads.lock().unwrap()[0].body.clone()).unwrap();
    assert!(
        body.contains(
            "{\"id\":\"gid://shopify/Product/1\",\"tags\":[\"sale\",\"a \\\"b\\\"\"]}\n\
             {\"id\":\"gid://shopify/Product/2\",\"tags\":[\"sale\",\"a \\\"b\\\"\"]}\n"
        ),
        "{body}"
    );

    assert_eq!(results.len(), 3);
    assert_eq!(results[0].index, Some(1));
    assert!(results[0].errors.is_empty());
    let refused = results[1].data.as_ref().unwrap().tags_add.as_ref().unwrap();
    assert_eq!(refused.user_errors[0].message, "Product does not exist");
    assert_eq!(results[1].index, Some(0));
    assert!(results[2].data.is_none());
    assert_eq!(results[2].index, None);
    assert_eq!(
        results[2].errors[0].message,
        "Variable $id of type ID! was provided invalid value"
    );
}

#[tokio::test]
async fn the_lines_of_an_inline_bulk_mutation_are_checked_before_anything_is_sent() {
    let (mock, _) = shopify_with("");
    let shopify = client(&mock);
    let deltas: [i64; 2] = [1, 1 << 40];
    let lines = deltas.map(|delta| {
        graphql!(
            bulk("mutation($input: InventoryAdjustQuantitiesInput!) {
                inventoryAdjustQuantities(input: $input) { userErrors { message } }
            }"),
            input = {
                name: "available",
                reason: "correction",
                changes: [{ delta: delta, inventoryItemId: "gid://shopify/InventoryItem/1", locationId: "gid://shopify/Location/1" }],
            },
        )
    });
    let error = shopify.bulk_mutation(lines, fast()).await.unwrap_err();
    let Error::InvalidValue { path, reason, .. } = &error else {
        panic!("{error:?}");
    };
    assert_eq!(path, "$input.changes[0].delta");
    assert_eq!(*reason, InvalidValueReason::IntOutOfRange);
    assert!(mock.requests().is_empty());

    // No line: nothing runs.
    let none: Vec<add_tags::Variables> = Vec::new();
    let mut results = shopify.bulk_mutation(none, fast()).await.unwrap();
    assert!(results.next_row().await.is_none());
    assert!(mock.requests().is_empty());
    let none: Vec<add_tags::Variables> = Vec::new();
    let error = shopify.start_bulk_mutation(none).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "bulk operation: a bulk mutation needs one line of variables at least"
    );
}

#[tokio::test]
async fn the_lines_of_a_bulk_mutation_run_one_mutation() {
    let (mock, _) = shopify_with("");
    let shopify = client(&mock);
    let line = graphql!(
        bulk("mutation($id: ID!) { tagsAdd(id: $id, tags: [\"a\"]) { userErrors { message } } }"),
        id = "gid://shopify/Product/1",
    );
    assert_eq!(line.line().unwrap(), r#"{"id":"gid://shopify/Product/1"}"#);

    // Each `graphql!` has its own types: only lines built by hand can mix
    // mutations.
    let lines = [
        BulkMutation::<Value>::__checked("mutation { a }", None, "2026-10"),
        BulkMutation::<Value>::__checked("mutation { b }", None, "2026-10"),
    ];
    let error = shopify.start_bulk_mutation(lines).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "bulk operation: line 2 of the bulk mutation was built from another mutation than line 1: a bulk operation runs one mutation"
    );
    assert!(mock.requests().is_empty());
}
