//! Against a real development store. Ignored by default: `mise run test:live`
//! runs them, with `SHOPIFY_TEST_SHOP` and either `SHOPIFY_TEST_ACCESS_TOKEN`
//! or `SHOPIFY_TEST_CLIENT_ID` and `SHOPIFY_TEST_CLIENT_SECRET`.
#![cfg(feature = "rustls")]

use std::time::Duration;

use shopify_api::bulk::{BulkOperationStatus, BulkOperationsFilter, BulkWaitOptions};
use shopify_api::{Auth, Shopify, graphql};

fn live() -> Shopify {
    let env = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    let shop = env("SHOPIFY_TEST_SHOP").expect("SHOPIFY_TEST_SHOP");
    let auth = match (
        env("SHOPIFY_TEST_ACCESS_TOKEN"),
        env("SHOPIFY_TEST_CLIENT_ID"),
        env("SHOPIFY_TEST_CLIENT_SECRET"),
    ) {
        (Some(token), _, _) => Auth::access_token(token),
        (None, Some(id), Some(secret)) => Auth::client_credentials(id, secret),
        _ => panic!(
            "set SHOPIFY_TEST_ACCESS_TOKEN, or SHOPIFY_TEST_CLIENT_ID and SHOPIFY_TEST_CLIENT_SECRET"
        ),
    };
    Shopify::new(shop, auth).expect("a valid shop")
}

#[tokio::test]
#[ignore = "calls a real store: mise run test:live"]
async fn reads_the_shop_with_a_typed_query() {
    let data = live()
        .execute(graphql!(
            "query Shop { shop { name myshopifyDomain currencyCode } }"
        ))
        .await
        .unwrap();
    assert!(!data.shop.name.is_empty());
    assert!(data.shop.myshopify_domain.ends_with(".myshopify.com"));
}

#[tokio::test]
#[ignore = "calls a real store: mise run test:live"]
async fn the_answer_carries_the_cost() {
    let response = live()
        .execute_response(graphql!("{ products(first: 1) { nodes { id } } }"))
        .await
        .unwrap();
    let cost = response.cost().expect("Shopify reports the cost");
    assert!(cost.throttle_status.maximum_available > 0.0);
}

#[tokio::test]
#[ignore = "calls a real store: mise run test:live"]
async fn downloads_the_schema_through_the_shop() {
    let schema = live().download_admin_schema().await.unwrap();
    assert!(schema.pointer("/data/__schema/types").is_some());
}

#[tokio::test]
#[ignore = "calls a real store: mise run test:live"]
async fn runs_a_bulk_query_to_its_end() {
    let shopify = live();
    let operation = shopify
        .run_bulk_query("{ products { edges { node { id title } } } }")
        .await
        .unwrap();
    let listed = shopify
        .bulk_operations(BulkOperationsFilter::new().first(5))
        .await
        .unwrap();
    assert!(listed.items.iter().any(|listed| listed.id == operation.id));

    let operation = shopify
        .wait_for_bulk_operation(
            &operation.id,
            BulkWaitOptions::new().timeout(Duration::from_secs(180)),
        )
        .await
        .unwrap();
    assert_eq!(operation.status, BulkOperationStatus::Completed);
    if let Some(url) = operation.url {
        let rows: Vec<serde_json::Value> = shopify.download_bulk_results(&url).await.unwrap();
        assert!(rows.iter().all(|row| row.get("id").is_some()));
    }
}
