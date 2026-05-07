use serde::Deserialize;
use std::time::Duration;

use shopify_api::{
    BulkConcurrencyOptions, BulkOperationsFilter, BulkWaitOptions, Shopify, ShopifyAPIError,
    ShopifyAuth, ShopifyBulkStatus, ShopifyConfig,
};

fn live_client() -> Option<Shopify> {
    let shop = std::env::var("SHOPIFY_TEST_SHOP").ok()?;
    let token = std::env::var("SHOPIFY_TEST_ACCESS_TOKEN").ok()?;
    Shopify::new(
        shop,
        ShopifyAuth::AccessToken(token),
        ShopifyConfig::default(),
    )
    .ok()
}

fn live_client_credentials_client() -> Option<Shopify> {
    let shop = std::env::var("SHOPIFY_TEST_SHOP").ok()?;
    let client_id = std::env::var("SHOPIFY_TEST_CLIENT_ID").ok()?;
    let client_secret = std::env::var("SHOPIFY_TEST_CLIENT_SECRET").ok()?;
    Shopify::new(
        shop,
        ShopifyAuth::client_credentials(client_id, client_secret),
        ShopifyConfig::default(),
    )
    .ok()
}

#[derive(Debug, Deserialize)]
struct ShopQuery {
    shop: Shop,
}

#[derive(Debug, Deserialize)]
struct Shop {
    name: String,
}

#[tokio::test]
async fn live_graphql_shop_query_when_env_is_present() -> Result<(), ShopifyAPIError> {
    let Some(shopify) = live_client() else {
        return Ok(());
    };

    let data: ShopQuery = shopify
        .graphql("query { shop { name } }", &serde_json::json!({}))
        .await?;

    assert!(!data.shop.name.is_empty());
    Ok(())
}

#[tokio::test]
async fn live_schema_download_when_env_is_present() -> Result<(), ShopifyAPIError> {
    let Some(shopify) = live_client() else {
        return Ok(());
    };

    let schema = shopify.download_admin_schema().await?;

    assert!(schema.pointer("/data/__schema").is_some());
    Ok(())
}

#[tokio::test]
async fn live_client_credentials_when_env_is_present() -> Result<(), ShopifyAPIError> {
    let Some(shopify) = live_client_credentials_client() else {
        return Ok(());
    };
    let token = shopify.access_token().await?;

    assert!(!token.is_empty());
    Ok(())
}

#[tokio::test]
async fn live_bulk_query_wait_list_and_download_when_env_is_present() -> Result<(), ShopifyAPIError>
{
    let Some(shopify) = live_client_credentials_client() else {
        return Ok(());
    };

    let payload = shopify
        .run_bulk_query(
            r#"{
                products {
                    edges {
                        node {
                            id
                            title
                        }
                    }
                }
            }"#,
        )
        .await?;

    assert!(
        payload.user_errors.is_empty(),
        "bulk query returned user errors: {:?}",
        payload.user_errors
    );

    let operation = payload
        .bulk_operation
        .expect("bulkOperationRunQuery should return a bulk operation");

    let listed = shopify
        .list_bulk_operations(BulkOperationsFilter {
            first: 5,
            ..BulkOperationsFilter::default()
        })
        .await?;
    assert!(listed.edges.iter().any(|edge| edge.node.id == operation.id));

    let operation = shopify
        .wait_for_bulk(
            &operation.id,
            BulkWaitOptions {
                poll_interval: Duration::from_secs(5),
                timeout: Some(Duration::from_secs(180)),
            },
        )
        .await?;

    assert_eq!(operation.status, ShopifyBulkStatus::Completed);

    if let Some(url) = operation.url {
        let rows: Vec<serde_json::Value> = Shopify::download_bulk_jsonl(&url).await?;
        for row in rows {
            assert!(row.get("id").is_some());
        }
    }

    Ok(())
}

#[tokio::test]
async fn live_five_concurrent_bulk_queries_when_enabled() -> Result<(), ShopifyAPIError> {
    if std::env::var("SHOPIFY_TEST_BULK_CONCURRENCY")
        .ok()
        .as_deref()
        != Some("1")
    {
        return Ok(());
    }

    let Some(shopify) = live_client_credentials_client() else {
        return Ok(());
    };

    let query = r#"{
        products(first: 1) {
            edges {
                node {
                    id
                    title
                }
            }
        }
    }"#;
    let payloads = shopify
        .run_bulk_queries(vec![query; 5], BulkConcurrencyOptions::default())
        .await?;
    let operations = payloads
        .into_iter()
        .map(|payload| {
            assert!(
                payload.user_errors.is_empty(),
                "bulk query returned user errors: {:?}",
                payload.user_errors
            );
            payload
                .bulk_operation
                .expect("bulkOperationRunQuery should return a bulk operation")
        })
        .collect::<Vec<_>>();

    assert_eq!(operations.len(), 5);

    let running_or_recent = shopify
        .list_bulk_operations(BulkOperationsFilter {
            first: 10,
            query: Some("type:QUERY".to_string()),
            ..BulkOperationsFilter::default()
        })
        .await?;
    for operation in &operations {
        assert!(running_or_recent
            .edges
            .iter()
            .any(|edge| edge.node.id == operation.id));
    }

    for operation in operations {
        let operation = shopify
            .wait_for_bulk(
                &operation.id,
                BulkWaitOptions {
                    poll_interval: Duration::from_secs(5),
                    timeout: Some(Duration::from_secs(180)),
                },
            )
            .await?;

        assert_eq!(operation.status, ShopifyBulkStatus::Completed);
    }

    Ok(())
}
