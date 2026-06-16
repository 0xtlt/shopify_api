# shopify_api

[![crates.io](https://img.shields.io/crates/v/shopify_api.svg)](https://crates.io/crates/shopify_api)
[![Documentation](https://docs.rs/shopify_api/badge.svg)](https://docs.rs/shopify_api)
[![MIT licensed](https://img.shields.io/crates/l/shopify_api.svg)](./LICENSE.txt)
[![CI](https://github.com/0xtlt/shopify_api/actions/workflows/ci.yml/badge.svg)](https://github.com/0xtlt/shopify_api/actions/workflows/ci.yml)

An ergonomic Shopify Admin GraphQL API client for Rust.

Version `0.10` is a breaking refactor for Shopify Admin API `2026-04` and newer:

- GraphQL-first client
- `2026-04` minimum API version
- static access tokens, client credentials tokens, and expiring offline tokens
- dynamic GraphQL schema download
- Bulk operations using the 2026 `bulkOperation(id:)` and `bulkOperations` APIs

## Install

```toml
[dependencies]
shopify_api = "0.10"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

## Basic GraphQL

```rust,no_run
use serde::Deserialize;
use shopify_api::{Shopify, ShopifyAuth, ShopifyConfig};

#[derive(Deserialize)]
struct ShopQuery {
    shop: Shop,
}

#[derive(Deserialize)]
struct Shop {
    name: String,
}

#[tokio::main]
async fn main() -> Result<(), shopify_api::ShopifyAPIError> {
    let shopify = Shopify::new(
        "my-shop",
        ShopifyAuth::AccessToken("shpat_...".to_string()),
        ShopifyConfig::default(),
    )?;

    let data: ShopQuery = shopify
        .graphql(
            "query { shop { name } }",
            &serde_json::json!({}),
        )
        .await?;

    println!("{}", data.shop.name);
    Ok(())
}
```

## Client Credentials

```rust,ignore
use shopify_api::{Shopify, ShopifyAuth, ShopifyConfig};

let shopify = Shopify::new(
    "my-shop",
    ShopifyAuth::client_credentials("client_id", "client_secret"),
    ShopifyConfig::default(),
)?;
```

The crate acquires and refreshes 24-hour client-credentials tokens automatically. Add a `TokenStore` in `ShopifyConfig` when your app needs to persist refreshed token data.

## Dynamic GraphQL Schema

Public Shopify schema:

```rust,no_run
# async fn example() -> Result<(), shopify_api::ShopifyAPIError> {
let schema = shopify_api::download_public_admin_schema("2026-04").await?;
# Ok(())
# }
```

```bash
cargo install shopify_api --features cli
shopify-api schema download \
  --public \
  --api-version 2026-04 \
  --out graphql.schema.json
```

This uses Shopify's public schema proxy:

```txt
https://shopify.dev/admin-graphql-direct-proxy/2026-04
```

The proxy is a GraphQL endpoint for introspection requests, not a static `GET` file.

Runtime:

```rust,no_run
use shopify_api::{Shopify, ShopifyAuth, ShopifyConfig};

# async fn example() -> Result<(), shopify_api::ShopifyAPIError> {
let shopify = Shopify::new(
    "my-shop",
    ShopifyAuth::AccessToken("shpat_...".to_string()),
    ShopifyConfig::default(),
)?;
let schema = shopify.download_admin_schema().await?;
# Ok(())
# }
```

CLI:

```bash
cargo install shopify_api --features cli
shopify-api schema download \
  --shop my-shop \
  --access-token "$SHOPIFY_ACCESS_TOKEN" \
  --out shop.graphql.schema.json
```

The crate no longer ships built-in Shopify schema JSON files. Use the CLI output as the `schema_path` for `graphql_client`.

## Bulk Operations

```rust,no_run
use shopify_api::{BulkWaitOptions, Shopify, ShopifyAuth, ShopifyConfig};

# async fn example() -> Result<(), shopify_api::ShopifyAPIError> {
let shopify = Shopify::new(
    "my-shop",
    ShopifyAuth::AccessToken("shpat_...".to_string()),
    ShopifyConfig::default(),
)?;

let payload = shopify
    .run_bulk_query("{ products { edges { node { id title } } } }")
    .await?;

let operation = payload.bulk_operation.unwrap();
let operation = shopify
    .wait_for_bulk(&operation.id, BulkWaitOptions::default())
    .await?;

if let Some(url) = operation.url {
    let rows: Vec<serde_json::Value> = Shopify::download_bulk_jsonl(&url).await?;
    println!("{} rows", rows.len());
}
# Ok(())
# }
```

## Changelog

See [CHANGELOG.md](CHANGELOG.md).
