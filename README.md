# shopify_api

[![crates.io](https://img.shields.io/crates/v/shopify_api.svg)](https://crates.io/crates/shopify_api)
[![Documentation](https://docs.rs/shopify_api/badge.svg)](https://docs.rs/shopify_api)
[![MIT/Apache-2 licensed](https://img.shields.io/crates/l/shopify_api.svg)](./LICENSE.txt)
[![CI](https://github.com/0xtlt/shopify_api/actions/workflows/ci.yml/badge.svg)](https://github.com/0xtlt/shopify_api/actions/workflows/ci.yml)
[![Issues](https://img.shields.io/github/issues/0xtlt/shopify_api)](https://img.shields.io/github/issues/0xtlt/shopify_api)

An ergonomic, Shopify API Client for Rust.

- GraphQL API support with automatic data deserialization
- [Changelog](CHANGELOG.md)

## Example

This asynchronous example uses [Tokio](https://tokio.rs) and enables some
optional features, so your `Cargo.toml` could look like this:

```toml
[dependencies]
shopify_api = "0.4"
tokio = { version = "1", features = ["full"] }
```

And then the code:

```rust,no_run
use shopify_api::*;
use shopify_api::utils::ReadJsonTreeSteps;
use serde::{Deserialize};

#[derive(Deserialize)]
struct Shop {
  name: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let shopify = Shopify::new("hello", "world", ShopifyAPIVersion::V2023_01, None);

  let graphql_query = r#"
    query {
      shop {
      name
     }
  }"#;

  let variables = serde_json::json!({});
  let json_finder = vec![ReadJsonTreeSteps::Key("data"), ReadJsonTreeSteps::Key("shop")];

  let shop: Shop = shopify.graphql_query(graphql_query, &variables, &json_finder).await.unwrap();
  Ok(())
}
```

## License

Licensed under MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)
