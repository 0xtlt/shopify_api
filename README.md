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
shopify_api = "0.7.0"
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
  let shopify = Shopify::new("hello", "world", String::from("2024-04"), None);

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

### Or with the `new` GraphQl Client!

```toml
[dependencies]
shopify_api = "0.7.0"
tokio = { version = "1", features = ["full"] }
graphql_client = "0.14.0"
```

```graphql
query GetShop {
  shop {
    name
  }
}
```

```rust,no_run
use shopify_api::*;
use graphql_client::GraphQLQuery;

#[derive(GraphQLQuery)]
#[graphql(
    schema_path = "./graphql.schema.json",
    query_path = "graphql/getShop.graphql",
    response_derives = "Debug"
)]
struct GetShop;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
  let shopify = Shopify::new("hello", "world", String::from("2024-04"), None);

  let shop_info = connector
        .shopify
        .post_graphql::<GetShop>(get_shop::Variables {})
        .await;

  Ok(())
}
```

### Download the graphql schema

#### You can download one from this repository

- [2024-04](./schemas/2024-04.json)

#### Or download it from the Shopify Graphql API with [the following command](./schema_dl.graphql)

> [!WARNING]
> Sometimes you'll get an error with the GraphQLQuery derive caused my a missing struct, most of the time, you can fix it by adding the missing struct by importing it from [the types import](./src/graphql/types.rs) or you can create a new struct with the same name as the missing one, and the derive will work.

## License

Licensed under MIT license ([LICENSE-MIT](LICENSE-MIT) or <http://opensource.org/licenses/MIT>)
