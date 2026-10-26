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
- **Typed variables and results**, input objects included, written as
  literals checked field by field, with no build script and no generated file.
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

Rust 1.88 or newer, and a Tokio runtime: the default transport and the token
renewals run on Tokio. The first build downloads the Admin schema of the latest
stable version into `graphql/admin.schema.json`, next to your `Cargo.toml`:
commit it. Later builds keep that version until you change it.

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
An enum value is written bare, `sortKey = TITLE`, and checked.

### Input objects

An input object is written as an object literal, checked against the schema
field by field while the crate compiles:

```rust,ignore
shopify.execute(graphql!(
    "mutation($product: ProductUpdateInput!) { productUpdate(product: $product) { userErrors { field message } } }",
    product = {
        id: product_id,                   // any Rust expression
        title: format!("{name} (promo)"),
        status: ACTIVE,                   // an enum value, checked
        descriptionHtml: null,            // sends null
        vendor?: new_vendor,              // an Option: None leaves the field out
        seo: { title: seo_title },        // a nested object
        metafields: [for m in &rows if !m.value.is_empty() => {
            namespace: "custom", key: &m.key, value: &m.value, type: "single_line_text_field",
        }],
    },
)).await?;
```

A field is written one of four ways:

| Field | Sends |
| --- | --- |
| `title: value` | The value |
| `title: null` | `null`: Shopify clears the field |
| `title?: option` | The value of `Some`; nothing for `None`, so Shopify keeps what it has |
| `title?: maybe` | A `shopify_api::Maybe`: `Value`, `Null`, or nothing for `Absent` |

`title: option` does not compile: sending `null` for `None` would clear the
field, so the error points to `title?:` or `title: null`. An unknown field, a
field given twice, a missing required field, `null` for a non-null field, an
unknown enum value and `?:` on a required field are compile errors at the
token. For an enum value known only at run time, `raw(value)` sends the
string unchecked: `status: raw(row.status)`.

`input!` builds an input object ahead of the call, in a loop for instance.
Every `input!` of one GraphQL type gives the same Rust type, so the values of
two call sites go in one `Vec`:

```rust,ignore
use shopify_api::input;

let mut metafields = Vec::new();
for row in &rows {
    if row.color.is_empty() { continue; }
    metafields.push(input!(MetafieldsSetInput {
        ownerId: &row.product_id, namespace: "custom", key: "color", value: &row.color,
        type: "single_line_text_field", compareDigest?: row.digest.as_deref(),
    }));
}
shopify.execute(graphql!(
    "mutation($metafields: [MetafieldsSetInput!]!) { metafieldsSet(metafields: $metafields) { userErrors { message } } }",
    metafields = metafields,
)).await?;
```

A value of another input type is refused, the message naming the place and
the GraphQL type it expects: `` an item of `$metafields` expects a `MetafieldsSetInput`, built with `input!(MetafieldsSetInput { … })` ``.
The type given may show only as `InputObject<N>`, `N` a hash of its name.

### Values Shopify would not read as written

A value is checked when it is built, in a literal, an `input!`, a module's
`Variables` or `Operation::variable`: an `Int` must fit in 32 bits, a `Float`
must be finite (JSON has no NaN or infinity, and `null` would clear the
field), and a non-null `JSON` must not be `null` at its top (`null` inside it
is fine). An integer literal out of range, alone, negated, in parentheses or
in a list `[…]`, is a compile error at the literal; `Some(…)`, `vec![…]` and
any other form are checked at run time. A value known at run time makes `execute` fail before anything is sent, with
`Error::InvalidValue`, which says where and why, never the value:
`` invalid value at `$input.lineItems[2].quantity`: Int out of 32-bit range ``.

### Named

`graphql! { pub mod name = "…"; … }` names the types, for functions and
sharing. One block holds several modules. `Variables` runs as it is, and each
input object the variables hold is a type of the module:

```rust,ignore
graphql! {
    pub mod metafields_set = "mutation MetafieldsSet($metafields: [MetafieldsSetInput!]!) {
        metafieldsSet(metafields: $metafields) { metafields { id key } userErrors { field message code } }
    }";
    pub mod shop_name = "query ShopName { shop { name } }";
}

fn color_metafields(rows: &[Row]) -> Vec<metafields_set::MetafieldsSetInput> {
    rows.iter()
        .map(|row| input!(MetafieldsSetInput { ownerId: &row.product_id, key: "color", value: &row.color }))
        .collect()
}

let data = shopify.execute(metafields_set::Variables { metafields: color_metafields(&rows) }).await?;
let shop = shopify.execute(shop_name::operation()).await?;
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

### Limits

- A literal and an `input!` are checked against the schema of the crate
  where they are written; values of `input!` from another crate have the same
  type for the same GraphQL name.
- There is no `if`/`else` value form: a Rust expression returning an `Option`,
  a `Maybe` or a value of `input!` covers it.
- rustc's own errors, outside those of the macros, show an input object as
  `InputObject<N>`, `N` a hash of its name.
- Enum fields of a module's `Variables` and `raw(…)` are strings nothing checks.
- Under `#![no_implicit_prelude]`, a literal or an `input!` that holds a
  Rust expression does not compile: its messages need `#[diagnostic::…]`,
  which that attribute hides. Constants, modules and variables of scalars do.

The full reference of `graphql!`, with the mapping of every GraphQL type and
the rules of literals, is
[on docs.rs](https://docs.rs/shopify_api/latest/shopify_api/macro.graphql.html).

## API versions

Every operation is checked against a schema, and **sent to the version of that
schema**. No schema ships with the crate: the first build downloads the one of
the latest stable version into `graphql/admin.schema.json`, and later builds
keep it, whatever Shopify releases since: a build gives the same result
whatever the date. Commit the file, so that builds without network access
have it. A library that uses `graphql!` must ship it: as a dependency, a
crate never downloads its schema, and `cargo package` fails without it.
`schema = "…"` in `[package.metadata.shopify_api]` names another schema file
for that package, relative to its `Cargo.toml` and inside the package. The details are in the
[`graphql!` docs](https://docs.rs/shopify_api/latest/shopify_api/macro.graphql.html#schema-and-api-version).

To choose the version, pin it in your `Cargo.toml` (`graphql!` downloads its
schema when the file holds another one):

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
follows the lines being read (the longest of the last few dozen), not the file.

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

// In your HTTP handler, on the raw body. `headers`: an `&http::HeaderMap`
// (axum, actix, warp), or any (name, value) pairs.
let webhook = Webhook::verify(&headers, body, client_secret)?;
match webhook.topic() {
    "orders/create" => {
        let order: MyOrder = webhook.payload()?;
    }
    "app/uninstalled" | "shop/redact" | "customers/redact" => {
        // The signature covers the body, not the headers: check the shop.
        webhook.verify_payload_shop()?;
    }
    _ => {}
}

// Make the shop's subscriptions exactly these.
shopify.sync_webhook_subscriptions(&[
    WebhookSubscriptionSpec::new("orders/create", "https://example.com/webhooks"),
    WebhookSubscriptionSpec::new("app/uninstalled", "https://example.com/webhooks"),
]).await?;
```

Only JSON webhooks verify, a body whose first byte is `{` (no whitespace or
byte order mark before it), and the subscriptions the crate creates or syncs
are JSON: the client secret also signs launch queries, app proxy requests and
session tokens, which an installer of the app can read and make look like XML,
or like JSON after some whitespace.

## OAuth and embedded apps

Feature `oauth`:

```rust,ignore
use shopify_api::oauth::{RequestedToken, ShopifyApp};

// The clients it builds save the tokens they renew.
let app = ShopifyApp::new(client_id, client_secret).with_token_store(MyDatabase);

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
| `testing` | `MockTransport`, and the signing helpers of tests. |
| `cli` | The `shopify-api` command. |
| `full` | Every feature but `native-tls`: `webhooks`, `oauth`, `testing`, `cli`, over rustls. |

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
