//! Against a real development store. Ignored by default: `mise run test:live`
//! runs them, with `SHOPIFY_TEST_SHOP` and either `SHOPIFY_TEST_ACCESS_TOKEN`
//! or `SHOPIFY_TEST_CLIENT_ID` and `SHOPIFY_TEST_CLIENT_SECRET`. The token
//! needs `read_products` and `write_products`: the typed bulk test creates a
//! product tagged `shopify-api-live-test` when the store has none of the
//! shape it reads, and reuses it on later runs.
#![cfg(feature = "rustls")]

use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;
use shopify_api::bulk::{BulkOperationStatus, BulkOperationsFilter, BulkWaitOptions};
use shopify_api::{Auth, Shopify, graphql};

graphql! {
    /// Two levels of nested connections, two sibling connections under a
    /// product, and the nodes of an interface (`Media`).
    mod live_catalog = bulk("{
        products { edges { node {
            id title
            variants { edges { node {
                id sku
                metafields { edges { node { id namespace key value } } }
            } } }
            media { edges { node { id mediaContentType ... on MediaImage { alt } } } }
        } } }
    }");

    mod seed_fixture = "mutation SeedLiveFixture($input: ProductSetInput!) {
        productSet(input: $input, synchronous: true) {
            product { id }
            userErrors { field message }
        }
    }";
}

/// The tag of the product the typed bulk test creates.
const FIXTURE_TAG: &str = "shopify-api-live-test";

/// A small image, stable for years, that Shopify can fetch.
const FIXTURE_IMAGE: &str =
    "https://upload.wikimedia.org/wikipedia/commons/4/47/PNG_transparency_demonstration_1.png";

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
        .start_bulk_query("{ products { edges { node { id title } } } }")
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

/// Starts `live_catalog`, waits for it, and reads its results twice: as
/// `BulkItems` and as raw JSONL lines.
async fn run_live_catalog(shopify: &Shopify) -> (Vec<live_catalog::Item>, Vec<Value>) {
    let started = shopify
        .start_bulk_query(live_catalog::operation())
        .await
        .unwrap();
    let operation = shopify
        .wait_for_bulk_operation(
            &started.id,
            BulkWaitOptions::new().timeout(Duration::from_secs(600)),
        )
        .await
        .unwrap();
    assert_eq!(
        operation.status,
        BulkOperationStatus::Completed,
        "{operation:?}"
    );
    let Some(url) = operation.url else {
        return (Vec::new(), Vec::new());
    };
    let items = shopify
        .download_bulk_items(&url)
        .await
        .unwrap()
        .collect()
        .await
        .expect("the typed reader places every line: Shopify kept the alias and the order");
    let lines = shopify.download_bulk_results(&url).await.unwrap();
    (items, lines)
}

fn has_the_shape(items: &[live_catalog::Item]) -> bool {
    items.iter().any(|product| {
        !product.media.is_empty()
            && product
                .variants
                .iter()
                .any(|variant| !variant.metafields.is_empty())
    })
}

async fn seed_fixture(shopify: &Shopify) {
    let metafield = |value: &str| {
        shopify_api::input!(MetafieldInput {
            namespace: "shopify_api_test",
            key: "note",
            type: "single_line_text_field",
            value: value,
        })
    };
    let small = metafield("small");
    let medium = metafield("medium");
    let data = shopify
        .execute(seed_fixture::Variables {
            input: shopify_api::input!(ProductSetInput {
                title: "shopify_api live test",
                tags: [FIXTURE_TAG],
                productOptions: [{ name: "Size", values: [{ name: "S" }, { name: "M" }] }],
                variants: [
                    {
                        optionValues: [{ optionName: "Size", name: "S" }],
                        price: "1.00",
                        metafields: [small],
                    },
                    {
                        optionValues: [{ optionName: "Size", name: "M" }],
                        price: "1.00",
                        metafields: [medium],
                    },
                ],
                files: [{ originalSource: FIXTURE_IMAGE, contentType: IMAGE, alt: "fixture" }],
            }),
        })
        .await
        .unwrap();
    let payload = data.product_set.expect("productSet answered");
    assert!(payload.user_errors.is_empty(), "{:?}", payload.user_errors);
}

/// What the typed reader relies on and Shopify's guide does not show: the
/// `_shopifyApiTypename: __typename` alias is kept in the JSONL, and the
/// order of `groupObjects` (each object right after its parent) holds at the
/// second level too. The items are compared with a regrouping of the raw
/// lines by `__parentId` alone.
#[tokio::test]
#[ignore = "calls a real store: mise run test:live"]
async fn a_typed_bulk_query_regroups_two_levels_and_sibling_connections() {
    let shopify = live();
    let (mut items, mut lines) = run_live_catalog(&shopify).await;
    if !has_the_shape(&items) {
        seed_fixture(&shopify).await;
        // The image is fetched in the background: wait until it is listed.
        for _ in 0..20 {
            (items, lines) = run_live_catalog(&shopify).await;
            if has_the_shape(&items) {
                break;
            }
            tokio::time::sleep(Duration::from_secs(15)).await;
        }
    }
    assert!(
        has_the_shape(&items),
        "no product with an image and a variant metafield, even after creating one tagged `{FIXTURE_TAG}`"
    );

    // The raw lines, regrouped by `__parentId` alone.
    let id = |line: &Value| line["id"].as_str().unwrap().to_string();
    let mut products = Vec::new();
    let mut kind_of = HashMap::new();
    let mut children: HashMap<String, Vec<(String, Option<String>)>> = HashMap::new();
    for line in &lines {
        match line.get("__parentId").and_then(Value::as_str) {
            None => {
                kind_of.insert(id(line), "Product");
                products.push(id(line));
            }
            Some(parent) => {
                let typename = line
                    .get("_shopifyApiTypename")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                if kind_of.get(parent) == Some(&"Product") {
                    // Two connections under a product: the alias tells them apart.
                    assert!(typename.is_some(), "no `_shopifyApiTypename`: {line}");
                    if typename.as_deref() == Some("ProductVariant") {
                        kind_of.insert(id(line), "ProductVariant");
                    }
                } else {
                    // Alone under its variant: not aliased.
                    assert_eq!(kind_of.get(parent), Some(&"ProductVariant"), "{line}");
                    assert!(typename.is_none(), "{line}");
                }
                children
                    .entry(parent.to_string())
                    .or_default()
                    .push((id(line), typename));
            }
        }
    }

    let ids = |list: &[(String, Option<String>)], variants: bool| -> Vec<String> {
        list.iter()
            .filter(|(_, typename)| (typename.as_deref() == Some("ProductVariant")) == variants)
            .map(|(id, _)| id.clone())
            .collect()
    };
    let none = Vec::new();
    assert_eq!(
        items.iter().map(|item| item.id.clone()).collect::<Vec<_>>(),
        products
    );
    for product in &items {
        let below = children.get(&product.id).unwrap_or(&none);
        let variants: Vec<String> = product.variants.iter().map(|v| v.id.clone()).collect();
        assert_eq!(variants, ids(below, true), "{}", product.id);
        let media: Vec<String> = product.media.iter().map(|m| m.id.clone()).collect();
        assert_eq!(media, ids(below, false), "{}", product.id);
        for variant in &product.variants {
            let below = children.get(&variant.id).unwrap_or(&none);
            let metafields: Vec<String> = variant.metafields.iter().map(|m| m.id.clone()).collect();
            assert_eq!(metafields, ids(below, false), "{}", variant.id);
        }
    }
}
