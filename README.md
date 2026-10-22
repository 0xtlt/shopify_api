# shopify_api

[![crates.io](https://img.shields.io/crates/v/shopify_api.svg)](https://crates.io/crates/shopify_api)
[![Documentation](https://docs.rs/shopify_api/badge.svg)](https://docs.rs/shopify_api)
[![MIT licensed](https://img.shields.io/crates/l/shopify_api.svg)](./LICENSE.txt)
[![CI](https://github.com/0xtlt/shopify_api/actions/workflows/ci.yml/badge.svg)](https://github.com/0xtlt/shopify_api/actions/workflows/ci.yml)

A Shopify Admin GraphQL client for Rust, whose operations are checked against
Shopify's schema while your crate compiles.

```rust,no_run
use shopify_api::{Auth, Shopify, graphql};

#[tokio::main]
async fn main() -> shopify_api::Result<()> {
    let shopify = Shopify::new("my-shop", Auth::access_token("shpat_…"))?;

    let data = shopify
        .execute(graphql!(
            "query Products($first: Int!) {
               products(first: $first) { nodes { id title totalInventory } }
             }",
            first = 10,
        ))
        .await?;

    for product in data.products.nodes {
        println!("{}: {:?} in stock", product.title, product.total_inventory);
    }
    Ok(())
}
```

`data.products.nodes` is a `Vec` of structs generated from the selection, and
`total_inventory` an `Option<i64>` because the schema says it may be null.

- **Checked operations.** A field, argument, enum value or variable that does
  not exist is a compile error, with a suggestion. A deprecated one is a
  compiler warning with Shopify's reason.
- **Typed variables and results**, input objects included, generated with no
  build script and no generated file.
- **Tokens handled**: static tokens, the client credentials grant, expiring
  offline tokens. Obtained, renewed before they expire, renewed once when
  Shopify refuses one, and saved where you say.
- **Throttling handled**: a throttled request waits for what its cost needs,
  then goes again. Queries are retried after a server error; mutations never
  run twice.
- **The rest of an app's Admin API work**: pagination, bulk operations,
  webhooks, OAuth and session tokens, a mock transport for tests, and any
  API version from one line of `Cargo.toml`.

## Install

```toml
[dependencies]
shopify_api = "1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Rust 1.88 or newer. The first build downloads the Admin schema of the latest
stable version into `graphql/admin.schema.json`, next to your `Cargo.toml`:
commit it.

## Operations

### Inline

`graphql!("…", name = value, …)` is an operation whose types are inferred:

```rust,ignore
let data = shopify
    .execute(graphql!(
        "query($id: ID!) { order(id: $id) { name displayFinancialStatus totalPriceSet { shopMoney { amount currencyCode } } } }",
        id = "gid://shopify/Order/1",
    ))
    .await?;

let order = data.order.expect("an order");
println!("{} {:?} {}", order.name, order.display_financial_status, order.total_price_set.shop_money.amount);
```

Values are converted to the variable's type: any integer for an `Int`, `&str`
or `String` for an `ID`, a bare value or an `Option` for a nullable variable,
a slice or an array for a list. A value of the wrong type does not compile.

### Named

`graphql! { pub mod name = "…"; }` names the types, for functions, sharing and
typed input objects:

```rust,ignore
graphql! {
    pub mod metafields_set = "mutation MetafieldsSet($metafields: [MetafieldsSetInput!]!) {
        metafieldsSet(metafields: $metafields) {
            metafields { id key }
            userErrors { field message code }
        }
    }";
}

let variables = metafields_set::Variables {
    metafields: vec![metafields_set::MetafieldsSetInput {
        owner_id: product_id.to_string(),
        namespace: Some("custom".to_string()),
        key: "color".to_string(),
        value: "blue".to_string(),
        ..Default::default()
    }],
};
let data: metafields_set::Data = shopify.execute(metafields_set::operation(variables)).await?;
```

### From a file

```rust,ignore
graphql! {
    pub mod order = file("graphql/order.graphql");
}
```

### Untyped

For an operation built at run time, `shopify.graphql(query, variables)`
returns `data` as a `serde_json::Value`, `graphql_as::<T>` reads it as your
type, and `graphql_raw` returns the whole answer, `errors` included.

The full reference of `graphql!`, with the mapping of every GraphQL type, is
[on docs.rs](https://docs.rs/shopify_api/latest/shopify_api/macro.graphql.html).

## API versions

Every operation is checked against a schema, and **sent to the version of that
schema**. No schema ships with the crate: `graphql!` downloads the one of your
version into `graphql/admin.schema.json`, and downloads again when the version
changes. Commit the file, so that builds without network access have it.

Out of the box, the version is **the latest stable one**: when Shopify
releases a new version (each quarter), the next build that recompiles your
crate moves to it, and a field Shopify removed becomes a compile error. To
stay on one version, declare it in your `Cargo.toml`:

```toml
[package.metadata.shopify_api]
api-version = "2026-07"
```

Your operations are then checked against 2026-07, and the client sends them to
the 2026-07 endpoint: nothing else to change. `ShopifyBuilder::api_version`
pins every request to one version instead.

The `shopify-api` command (`cargo install shopify_api --features cli`)
refreshes the file (`shopify-api schema download`, or `--shop` for the schema
a token sees), and `shopify-api check graphql/*.graphql` checks `.graphql`
files the same way, for CI or for operations used by other tools.

## Tokens

```rust,ignore
use shopify_api::{Auth, Shopify, TokenData};

// A custom app's Admin API token, or an offline token stored at installation.
let shopify = Shopify::new("my-shop", Auth::access_token("shpat_…"))?;

// The client credentials grant: tokens of 24 hours for a store of the
// organization that owns the app, asked for and renewed by the client.
let shopify = Shopify::new("my-shop", Auth::client_credentials(client_id, client_secret))?;

// An expiring offline token, renewed with its refresh token and saved.
let shopify = Shopify::builder("my-shop")
    .auth(Auth::expiring_token(client_id, client_secret, stored_token))
    .token_store(MyDatabase)
    .build()?;
```

Clones of a client share its token: one renewal serves them all, however many
requests wait for it.

## Rate limits and retries

A request Shopify throttles is sent again when the bucket holds its cost again,
computed from `extensions.cost`. A `429` waits for its `Retry-After` (seconds or an HTTP date), even past
`max_delay` (which caps only the backoff the client computes); a `Retry-After`
longer than `max_retry_after` (five minutes by default) is not waited for, and
ends in `Error::Throttled` with that wait in `retry_after`. A query
is also retried after a `5xx` or a network failure, with exponential backoff;
a mutation is not, since it may have run.

```rust,ignore
use std::time::Duration;
use shopify_api::RetryPolicy;

let shopify = Shopify::builder("my-shop")
    .access_token(token)
    .retry(RetryPolicy::new(6).with_max_delay(Duration::from_secs(10)))
    .build()?;

let response = shopify.execute_response(operation).await?;
println!("{:?}", response.cost()); // requested and actual cost, what is left
```

## Pagination

```rust,ignore
use shopify_api::Page;

let products = shopify
    .paginate(
        |after| graphql!(
            "query($after: String) { products(first: 250, after: $after) { nodes { id title } pageInfo { hasNextPage endCursor } } }",
            after = after,
        ),
        |data| Page::new(data.products.nodes, data.products.page_info.has_next_page, data.products.page_info.end_cursor),
    )
    .await?;
```

## Bulk operations

```rust,ignore
use shopify_api::bulk::BulkWaitOptions;

// Start, wait, download, read each line.
let rows: Vec<MyProduct> = shopify
    .bulk_query_rows("{ products { edges { node { id title } } } }", BulkWaitOptions::default())
    .await?;

// One mutation per item, uploaded as JSONL first.
let operation = shopify.run_bulk_mutation(
    "mutation($product: ProductUpdateInput!) { productUpdate(product: $product) { userErrors { message } } }",
    &inputs,
).await?;
```

`run_bulk_query`, `wait_for_bulk_operation`, `bulk_operations`,
`cancel_bulk_operation` and `download_bulk_results` give each step on its own.

### Large exports: stream the rows

`bulk_query_rows` holds every row in a `Vec`. For an export of gigabytes,
`bulk_query_stream` reads the file line by line as it downloads: memory
follows the line being read, not the file.

```rust,ignore
let mut products = shopify
    .bulk_query_stream::<MyProduct>("{ products { edges { node { id title } } } }", BulkWaitOptions::default())
    .await?;
while let Some(product) = products.next_row().await {
    let product = product?; // a line that does not read: skip it, or stop
    save(product).await?;
}
```

`BulkRows` is also a `futures_core::Stream`. A line that does not read as
the type is an error and the stream goes on; a download that fails, or a
line longer than `max_line_length` (64 MiB by default), ends it.

Nested connections write their objects on lines of their own, after their
parent. `bulk_query_grouped::<Product, Variant>` runs the query with
`groupObjects` and yields each product with its variants, one product in
memory at a time. Every object that has a nested connection must select
`id`, without an alias: Shopify refuses the query otherwise.


```rust,ignore
let mut products = shopify
    .bulk_query_grouped::<MyProduct, MyVariant>(
        "{ products { edges { node { id title variants { edges { node { id sku } } } } } } }",
        BulkWaitOptions::default(),
    )
    .await?;
while let Some(group) = products.next_group().await {
    let (product, variants) = group?;
}
```

`download_bulk_results_stream` and `download_bulk_results_grouped` read the
results of an operation you ran yourself. For a bulk mutation over more
items than you want in memory, `run_bulk_mutation_iter` takes an iterator:
each item is serialized as it is taken.

Use the `Vec` methods when the results fit in memory, the streams otherwise.

## Webhooks

Feature `webhooks`:

```rust,ignore
use shopify_api::webhooks::{Webhook, WebhookSubscriptionSpec};

// In your HTTP handler, on the raw body.
let webhook = Webhook::verify(headers, body, client_secret)?;
if webhook.topic() == "orders/create" {
    let order: MyOrder = webhook.payload()?;
}

// Make the shop's subscriptions exactly these.
shopify.sync_webhook_subscriptions(&[
    WebhookSubscriptionSpec::new("orders/create", "https://example.com/webhooks"),
    WebhookSubscriptionSpec::new("app/uninstalled", "https://example.com/webhooks"),
]).await?;
```

## OAuth and embedded apps

Feature `oauth`:

```rust,ignore
use shopify_api::oauth::{RequestedToken, ShopifyApp};

let app = ShopifyApp::new(client_id, client_secret);

// Embedded app: the session token of each request, exchanged for a token.
let session = app.verify_session_token(bearer)?;
let token = app.exchange_session_token(&session, RequestedToken::ExpiringOffline).await?;
let shopify = app.client(session.shop(), token)?;

// Install link and callback, admin launches, app proxies.
let link = app.authorization_url(&shop, &request);
let callback = app.verify_query(query_string)?;
let token = app.exchange_code(&callback).await?;
let proxied = app.verify_app_proxy(query_string)?;
```

## Testing your code

Feature `testing`: a `MockTransport` records what the client sends and answers
what the test says, with no network.

```rust,ignore
use shopify_api::testing::{MockTransport, client};

let mock = MockTransport::new();
mock.push_data(serde_json::json!({ "shop": { "name": "Test shop" } }));
let shopify = client(&mock);

assert_eq!(my_code_under_test(&shopify).await?, "Test shop");
assert_eq!(mock.graphql_requests()[0].operation_name.as_deref(), Some("Shop"));
```

## Features

| Feature | |
| --- | --- |
| `rustls` (default) | The default transport over rustls. |
| `native-tls` | The same over the platform's TLS. |
| `webhooks` | Webhook verification, parsing and subscriptions. |
| `oauth` | Install links, callbacks, token exchange, session tokens, app proxies. |
| `testing` | `MockTransport`. |
| `cli` | The `shopify-api` command. |
| `full` | All of the above. |

Without `rustls` or `native-tls`, give the client your own `Transport`.

## Upgrading from 0.10

See [MIGRATION.md](MIGRATION.md).

## Development

The toolchain and the tasks are in `mise.toml`:

```bash
mise install
mise run check          # what CI runs: fmt, clippy, tests, docs
mise run schema:test     # download the schema the tests use again (version in crates/shopify_api/Cargo.toml)
mise run schema:internal # cut it down to the crate's own operations, graphql/internal.schema.json
mise tasks              # every task
```

The tests never reach Shopify: they script it with `MockTransport`.
`mise run test:live` runs a few against a real development store.

## License

MIT
