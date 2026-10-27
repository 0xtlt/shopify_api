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
renewals run on Tokio. `graphql!` checks operations against the Admin schema
in `graphql/admin.schema.json`, next to your `Cargo.toml`: write it once with
`shopify-api schema download` (`cargo install shopify_api --features cli`),
and commit it. Later builds keep that version until you change it. With the
`schema-download` feature, the first build downloads it instead.

## Operations

`graphql!("…", name = value, …)` is an operation whose types are inferred.
Values are converted to the variable's type, and an input object is written
as an object literal, checked field by field while the crate compiles:

```rust,ignore
shopify.execute(graphql!(
    "mutation($product: ProductUpdateInput!) { productUpdate(product: $product) { userErrors { field message } } }",
    product = {
        id: product_id,                   // any Rust expression
        status: ACTIVE,                   // an enum value, checked
        descriptionHtml: null,            // sends null
        vendor?: new_vendor,              // an Option: None leaves the field out
        seo: { title: seo_title },        // a nested object
    },
)).await?;
```

`graphql! { pub mod name = "…"; }` names the types, for functions and
sharing; `file("graphql/order.graphql")` reads an operation from a file;
`input!(MetafieldsSetInput { … })` builds an input object ahead of the call.
For an operation built at run time, `shopify.graphql(query, variables)`
returns `data` as a `serde_json::Value`.

The full reference of `graphql!` is
[on docs.rs](https://docs.rs/shopify_api/latest/shopify_api/macro.graphql.html):
the mapping of every GraphQL type, the forms of a field, the values checked
at compile time and at run time, `crate =` for a framework that re-exports
`shopify_api`, and the limits.

## API versions

Every operation is checked against a schema, and **sent to the version of that
schema**. No schema ships with the crate: `shopify-api schema download`
writes the one of the latest stable version into `graphql/admin.schema.json`,
and builds keep it, whatever Shopify releases since: a build gives the same
result whatever the date. A file of the version the crate asks for is always
used as it is. Builds never use the network, unless the `schema-download`
feature lets `graphql!` download a missing or outdated file itself. Commit
the file. A library that uses `graphql!` must ship it: as a dependency, a
crate never downloads its schema, and `cargo package` fails without it.
`schema = "…"` in `[package.metadata.shopify_api]` names another schema file
for that package, relative to its `Cargo.toml`: a relative path with `/` and portable names (no `:`, `\`, `<>"|?*` or control character, no name ending in `.` or a space, no device name such as `CON`). The details are in the
[`graphql!` docs](https://docs.rs/shopify_api/latest/shopify_api/macro.graphql.html#schema-and-api-version).

To choose the version, pin it in your `Cargo.toml`, then run
`shopify-api schema download` again (with `schema-download`, the next build
does it):

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

When Shopify refuses a token, `Error::Auth` says how, without a message to
parse: `error.token_endpoint_status()` (`Some(400)`), `error.oauth_code()`
(`Some(OAuthErrorCode::InvalidClient)`, `None` when the body has no code) and
`error.is_held_auth_failure()` (the failure of a renewal another request ran).
A `4xx` other than `408` and `429` with a code will not pass by itself; a
`5xx`, `408`, `429` or no code may.

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

`graphql!(bulk(…))` checks a bulk query against the schema and Shopify's
bulk rules (one top-level connection, five connections at most, two levels
deep, `edges { node }`, an `id` on each parent…), and reads its results as
generated types: each nested connection is a `Vec` of its nodes, put back
under its parent as the file downloads, one product in memory at a time.

```rust,ignore
use shopify_api::bulk::BulkWaitOptions;

graphql! {
    mod catalog = bulk("query($filter: String) {
        products(query: $filter) { edges { node {
            id title
            variants { edges { node { sku } } }
        } } }
    }");
}

let mut products = shopify
    .bulk_query(catalog::Variables { filter: Some("status:active".into()) }, BulkWaitOptions::default())
    .await?;
while let Some(product) = products.next_item().await {
    let product = product?;
    println!("{}: {} variants", product.title, product.variants.len());
}
```

Shopify takes no variables for a bulk query: they are written into its text
as GraphQL literals. A bulk mutation, `bulk("mutation …")`, runs over an
iterator of its `Variables` with `bulk_mutation`, which checks every line
before uploading them and reads a typed result per line. See the
[documentation of `graphql!`](https://docs.rs/shopify_api/latest/shopify_api/macro.graphql.html#bulk-operations)
for the rules, and what is not checked.

The untyped methods run any text:

```rust,ignore
use shopify_api::bulk::BulkWaitOptions;

// Start, wait, download, read each line.
let rows: Vec<MyProduct> = shopify
    .bulk_query_rows("{ products { edges { node { id title } } } }", BulkWaitOptions::default())
    .await?;

// Start only: one mutation per item, uploaded as JSONL first.
let operation = shopify.start_bulk_mutation_text(
    "mutation($product: ProductUpdateInput!) { productUpdate(product: $product) { userErrors { message } } }",
    &inputs,
).await?;
```

One verb per meaning: `start_bulk_*` only starts an operation and returns
it; `bulk_query*` and `bulk_mutation` start, wait and read. `start_bulk_query`
takes a text or a `graphql!(bulk(…))` query; `start_bulk_mutation` takes typed lines, and
`start_bulk_mutation_text` a text and its variables. With
`wait_for_bulk_operation`, `bulk_operations`, `cancel_bulk_operation` and
`download_bulk_results`, each step is on its own.

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
parent: declare the query with `graphql!(bulk(…))` to get each product with
its variants, one product in memory at a time.

`download_bulk_results_stream` and `download_bulk_items` read the results of
an operation you ran yourself. For a bulk mutation over more
items than you want in memory, `start_bulk_mutation_text` takes an iterator:
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

// Or, once the token is stored: verify the session, then load the shop's token.
let session = app.verify_session_token(bearer)?;
let shopify = app.client(session.shop(), my_database.offline_token(session.shop())?)?;

// A checkout or customer account UI extension: the shop, and the customer if logged in.
let extension = app.verify_extension_session_token(bearer)?;
let customer = extension.customer(); // Some("gid://shopify/Customer/…")

// Install link and callback, admin launches, app proxies.
let link = app.authorization_url(&shop, &request);
let callback = app.verify_query(query_string)?;
let token = app.exchange_code(&callback).await?;
let proxied = app.verify_app_proxy(query_string)?;
```

`verify_session_token` takes only the tokens of Shopify admin (`dest` and
`iss` exactly `https://{shop}.myshopify.com` and its `/admin`), which Shopify
gives a staff member: they may open the admin side of the app, such as the
stored offline token of the shop. The tokens of checkout and customer account
UI extensions are signed with the same secret, but any buyer of the shop gets
one, logged in or not: `verify_extension_session_token` verifies them, as an
`ExtensionSessionToken` that does not exchange for a token, so that one can
never pass for the other.

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
| `schema-download` | `graphql!` downloads a missing or outdated schema file while compiling. |
| `full` | Every feature but `native-tls`: `webhooks`, `oauth`, `testing`, `cli`, `schema-download`, over rustls. |

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
`mise run test:live` runs a few against a real development store:

```bash
SHOPIFY_TEST_SHOP=my-dev-store SHOPIFY_TEST_ACCESS_TOKEN=shpat_… mise run test:live
# or SHOPIFY_TEST_CLIENT_ID and SHOPIFY_TEST_CLIENT_SECRET instead of the token
```

The token needs `read_products` and `write_products`. One of them runs a
typed bulk query, `products { variants { metafields } media }`, and compares
its items with the raw JSONL regrouped by `__parentId`: it confirms what
Shopify's guide does not show and the typed reader relies on, that the
`_shopifyApiTypename` alias is kept in the file and that `groupObjects`
writes each object right after its parent at the second level too. When
the store has no product with an image and a variant metafield, it creates
one, tagged `shopify-api-live-test`, and reuses it later.

## License

MIT
