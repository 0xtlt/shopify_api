# Upgrading from 0.10 to 1.0

1.0 does most of what 0.10 did, under new names, and adds operations checked
at compile time. A few things are gone without a replacement in the crate:
the typed webhook payloads and the warp wrapper (a handler is a few lines,
see [Webhooks](#webhooks)), the concurrent bulk helpers, and `utils`. Most
code changes in three places: building the client, the error type, and
GraphQL calls.

From 0.9 or older (`rest_query` and the REST module): 0.10 removed REST and
moved to GraphQL; see the 0.10.0 entry of [CHANGELOG.md](CHANGELOG.md), then
this guide.

## Building the client

```rust,ignore
// 0.10
let shopify = Shopify::new("my-shop", ShopifyAuth::AccessToken(token), ShopifyConfig::default())?;

// 1.0
let shopify = Shopify::new("my-shop", Auth::access_token(token))?;
```

| 0.10 | 1.0 |
| --- | --- |
| `ShopifyAuth::AccessToken(token)` | `Auth::access_token(token)` |
| `ShopifyAuth::client_credentials(id, secret)` | `Auth::client_credentials(id, secret)` |
| `ShopifyAuth::ClientCredentials { current_token: Some(token), .. }` | `Auth::client_credentials(id, secret).with_token(token)` |
| `ShopifyAuth::expiring_offline_token(id, secret, token)` | `Auth::expiring_token(id, secret, token)` |
| `ShopifyConfig { api_version, .. }` | `Shopify::builder(shop).api_version(ApiVersion::V2026_07)` |
| `ShopifyConfig { token_store, .. }` | `.token_store(store)` |
| `ShopifyConfig { token_refresh_leeway, .. }` | `.refresh_leeway(duration)` (a `std::time::Duration`) |
| `ShopifyConfig { user_agent, .. }` | `.user_agent(text)` |
| `ShopifyConfig { shared_secret, .. }` | Gone: pass the secret to `webhooks::Webhook::verify` |
| `ApiVersion::new(text)`, from an `impl Into<String>` | `ApiVersion::new(&str)`, `text.parse::<ApiVersion>()`, or `ApiVersion::V2026_07` |
| `ApiVersion::try_from(string)` (a `String`) | `ApiVersion::try_from(string.as_str())` |
| `version.as_str()` | `version.to_string()` (`Display`) |
| `shopify.api_version` (a public field) | `shopify.api_version()` |
| `DEFAULT_API_VERSION`, `MIN_API_VERSION` | `ApiVersion::DEFAULT` (2026-10), `ApiVersion::MINIMUM` |
| `VERSION` | `USER_AGENT` |
| `shopify.replace_auth(auth)` | Build another client: see below |
| `shopify.get_shop()`, `shop_domain()` | `shopify.shop()`, a `ShopDomain` |
| `shopify.get_query_url()` | `shopify.graphql_url(version)` |
| `shopify.token_url()` | Gone: the client asks for its tokens itself |
| `shopify.request_client_credentials_token(id, secret)` | `Shopify::new(shop, Auth::client_credentials(id, secret))?`, then `shopify.access_token().await?` (the token, obtained if need be) or `shopify.token().await` (the whole `TokenData`) |
| `shopify.refresh_token_with_credentials(id, secret, refresh)` | `Auth::expiring_token(id, secret, token)`: the client refreshes before expiry and on a `401`, and saves with its `TokenStore` |

`replace_auth` swapped the token of a client in place. A client's
credentials are now fixed: when a shop reinstalls the app, or its token
changes otherwise, build a new client with the new token, and replace the old
one wherever it is held (its clones keep the old token).

`TokenStore::save_token(shop: &str, token: TokenData)` is
`TokenStore::save(shop: &ShopDomain, token: &TokenData)`, and returns a boxed
error of yours; `TokenStoreFuture` is `transport::BoxFuture`. While a save
fails, the client sends nothing: each call tries the save again first, and
fails with `Error::TokenStore`. `TokenData` has the same fields, two more for
online tokens, and reads the JSON 0.10 stored. It is `#[non_exhaustive]`: build
it with `TokenData::new(token)` (0.10's `TokenData::never_expiring`), then set
its fields, instead of a struct literal. `expires_within` takes a
`std::time::Duration`.

The shop must now be `<name>` or `<name>.myshopify.com` (with or without
`https://`): 0.10 accepted any text, `http://` included, and appended
`.myshopify.com`. An `http://` address is refused.

**Retries are on by default.** 0.10 returned `Throttled` at the first
throttle. 1.0 retries up to four times: a throttled request waits for what
its cost needs, a `429` for its `Retry-After` (up to five minutes), a query
for a backoff after a `5xx` or a network failure. To keep 0.10's behaviour:
`Shopify::builder(shop).retry(RetryPolicy::none())`.

Without a pinned version, an operation of `graphql!` goes to the version its
schema was checked against (the `api-version` of your `Cargo.toml`, else the
one of the schema file, else the latest on the first build), and an untyped
call to 2026-10. 0.10 sent everything to 2026-04 by default: pin it with
`.api_version(ApiVersion::V2026_04)` to keep that.

## Errors

`ShopifyAPIError` is `Error`, and `shopify_api::Result<T>` is available.

| 0.10 | 1.0 |
| --- | --- |
| `ConnectionFailed(reqwest::Error)` | `Transport { url, source }` |
| An `error_for_status` failure | `Http { status, url, content_type, request_id, raw_body }`: it displays the status, the address, the content type and the length of the body, never the body; `error.raw_http_body()` gives the whole body, secrets included; `error.status()`, `error.is_unauthorized()` |
| `Throttled` | `Throttled { retries, cost, retry_after }`, once the retries are spent or when `Retry-After` exceeds `max_retry_after` |
| `GraphqlErrors(Vec<GraphqlError>)` | `Graphql(GraphqlErrors { errors, data })` |
| `JsonParseError`, `NotWantedJsonFormat`, `NotJson`, `ResponseBroken` | `Decode { path, message, detail }`: a body that is not JSON, or `data` that does not fit its type. `path` and `message` hold no text of the answer (a map key is `[key]`); serde's message is `error.sensitive_detail()` |
| Variables that do not serialize | `Variables { message, detail }`: what a `Serialize` implementation wrote is `error.sensitive_detail()`, not the message |
| A `Float` that is NaN or infinite, an `Int` beyond 32 bits, `null` for a non-null `JSON`, sent as they were | `InvalidValue { path, reason, detail }`, before anything is sent: `` invalid value at `$product.metafields[2].value`: not a finite Float `` |
| `Authentication(text)` | `Auth { message, detail }`: what the token endpoint wrote is `error.sensitive_detail()` |
| `MissingGraphqlData` | Gone: an answer without `data` (and without `errors`) gives `Value::Null` to `graphql`; `graphql_as` and `execute` then fail with `Decode` unless the type reads `null` |
| `InvalidHeaderValue`, `Other` | Gone |

`Error` no longer converts from `reqwest::Error` or `serde_json::Error`: a
`?` on a `serde_json` result in a function that returns
`shopify_api::Result` needs your own error type, or `map_err`.

## GraphQL calls

```rust,ignore
// 0.10: a struct per answer, written by hand, and a json! of variables.
#[derive(Deserialize)]
struct ShopQuery { shop: Shop }
#[derive(Deserialize)]
struct Shop { name: String }
let data: ShopQuery = shopify.graphql("query { shop { name } }", &json!({})).await?;

// 1.0: checked when the crate compiles, typed from the selection.
let data = shopify.execute(graphql!("query { shop { name } }")).await?;
println!("{}", data.shop.name);

// 1.0, still untyped: `graphql_as` reads `data` as your type.
let data: ShopQuery = shopify.graphql_as("query { shop { name } }", ()).await?;
```

| 0.10 | 1.0 |
| --- | --- |
| `graphql::<T>(query, &variables)` | `graphql_as::<T>(query, variables)`, or `execute(graphql!(…))` |
| `graphql_raw(query, &variables)` | `graphql_raw(query, variables)`, a `RawResponse` |
| `graphql_at_path(query, &variables, &[Key("a"), Key("b")])` | `graphql(…)` then `value.pointer("/a/b")`, or a typed operation |
| `post_graphql::<Q>(variables)` (`graphql-client` feature) | `execute(graphql!(…))`: no schema file or build step to keep |
| `types::{Decimal, DateTime, JSON, UnsignedInt64}` | Gone: `graphql!` maps every scalar |

Variables are now anything that serializes to an object, by value or by
reference, or `()` for none.

The untyped calls send no `operationName`. To name one, for Shopify's logs:
`shopify.execute(Operation::<Value>::new(query).with_name("Name"))`.

| 0.10 | 1.0 |
| --- | --- |
| `GraphqlResponse<T>` | `RawResponse` (`data` as a `Value`), or `Response<D>` from `execute_response` |
| `GraphqlError` | `GraphqlError`, with `message`, `locations`, `path`, `extensions` and `code()` |
| `shopify_api::graphql::*` (the module) | The module is private: `shopify_api::graphql` is the macro, the types are at the root |

With `graphql!`, an input object is a literal checked against the schema,
in place of a `json!` value:

```rust,ignore
// 0.10
shopify.graphql::<Value>(mutation, &json!({ "product": { "id": id, "title": title } })).await?;

// 1.0
shopify.execute(graphql!(
    "mutation($product: ProductUpdateInput!) { productUpdate(product: $product) { userErrors { message } } }",
    product = { id: id, title: title },
)).await?;
```

## Bulk operations

| 0.10 | 1.0 |
| --- | --- |
| `run_bulk_query(query)` → `BulkOperationPayload` | `run_bulk_query(query)` → `BulkOperation`; user errors are `Error::UserErrors` |
| `run_bulk_query_with_grouping(query, group)` | `run_bulk_query_grouped(query, group)` |
| `run_bulk_mutation(mutation, staged_upload_path)` | `run_bulk_mutation_staged(mutation, path, None)`, or `run_bulk_mutation(mutation, &rows)`, which uploads the rows |
| `stage_upload_jsonl(&rows)` | `stage_bulk_variables(&rows)` |
| `get_bulk_operation(id)` | `bulk_operation(id)` |
| `list_bulk_operations(BulkOperationsFilter { .. })` | `bulk_operations(BulkOperationsFilter::new().first(10))`, a `Page<BulkOperation>` |
| `wait_for_bulk(id, BulkWaitOptions { .. })` | `wait_for_bulk_operation(id, BulkWaitOptions::new().timeout(…))` |
| `Shopify::download_bulk_jsonl(url)` | `shopify.download_bulk_results(url)`, or `bulk_query_rows` for the whole cycle; `download_bulk_results_stream(url)` and `bulk_query_stream` read the rows as they download |
| `run_bulk_queries`, `run_bulk_mutations`, `BulkConcurrencyOptions` | Gone: call the single versions in a loop |
| `ShopifyBulkOperation`, `ShopifyBulkStatus` | `bulk::BulkOperation`, `bulk::BulkOperationStatus` |
| `ShopifyBulkErrorCode` | `BulkOperation::error_code`, a `String` |
| `BulkOperationPayload` | `BulkOperation`, its `userErrors` as `Error::UserErrors` |
| `BulkOperationConnection`, `BulkOperationEdge`, `PageInfo` | `Page<BulkOperation>` |
| `StagedUploadResource`, `StagedUploadParameter`, `StagedMediaUploadTarget`, `StagedUploadsCreateInput` | Internal: `stage_bulk_variables` does the upload |
| `ShopifyUserError` | `UserError` |
| `parse_jsonl(&str)` | `bulk::parse_jsonl(&[u8])`: `parse_jsonl(text.as_bytes())` |

`object_count`, `root_object_count` and `file_size` are numbers, and
`created_at` and `completed_at` are `chrono::DateTime<Utc>`. A failed operation
is an `Error::Bulk` whose `operation` holds it, with its `error_code` and
`partial_data_url`; a wait that ran out is an `Error::Timeout` whose `id` is
the operation's, which keeps running.

## Webhooks

The `webhooks` feature verifies and reads webhooks without a web framework, so
the `warp-wrapper` feature, `Shopify::warp_wrapper` and the `ShopifyWebhook`
payload structs (`Order`, `Customer`, `InventoryItem`, `InventoryLevel`,
`Address`…) are gone: declare the fields you read in a struct of yours. With
warp, what `warp_wrapper` did:

```rust,ignore
use bytes::Bytes;
use serde::Deserialize;
use shopify_api::webhooks::Webhook;
use warp::http::{HeaderMap, StatusCode};
use warp::Filter;

#[derive(Deserialize)]
struct Order {
    id: u64,
    name: String,
}

let secret = std::env::var("SHOPIFY_API_SECRET")?;
let webhooks = warp::path("webhooks")
    .and(warp::post())
    .and(warp::header::headers_cloned())
    .and(warp::body::bytes())
    .map(move |headers: HeaderMap, body: Bytes| {
        // Checks the signature, and reads the topic, the shop and the IDs.
        let Ok(webhook) = Webhook::verify(&headers, body, &secret) else {
            return StatusCode::UNAUTHORIZED;
        };
        match webhook.topic() {
            "orders/create" => match webhook.payload::<Order>() {
                Ok(order) => println!("{} on {}", order.name, webhook.shop()),
                Err(_) => return StatusCode::BAD_REQUEST,
            },
            "app/uninstalled" => {
                // The shop of the headers is not signed: check the payload's.
                if webhook.verify_payload_shop().is_err() {
                    return StatusCode::UNAUTHORIZED;
                }
                // … forget the shop.
            }
            _ => {}
        }
        StatusCode::OK
    });
```

With axum, `async fn(headers: HeaderMap, body: Bytes)` takes the same two
values (see the docs of `shopify_api::webhooks`). `Webhook::verify` requires
the `X-Shopify-Topic` and `X-Shopify-Shop-Domain` headers, which Shopify always
sends, and only verifies JSON webhooks, whose body starts with `{` as its
first byte, with no whitespace or byte order mark before it: the client
secret also signs texts an installer of the app can read and make look like
XML, or like JSON after some whitespace. `sync_webhook_subscriptions` updates an XML subscription of a
desired topic and address to JSON;
`webhooks::verify_hmac(secret, body, header)` checks the signature alone.

| 0.10 | 1.0 |
| --- | --- |
| `shopify.verify_hmac(data, header)` | `webhooks::verify_hmac(secret, data, header)` |
| `list_webhooks()` | `webhook_subscriptions()`, every page |
| `add_webhook(address, topic, format)` | `create_webhook_subscription(&WebhookSubscriptionSpec::new(topic, address))`, always JSON |
| `delete_webhook(id)` | `delete_webhook_subscription(id)` |
| `webhook_auto_config(vec![(address, topic, format)])` | `sync_webhook_subscriptions(&[WebhookSubscriptionSpec::new(topic, address)])`, always JSON, which returns what it did |
| `delete_webhook(id)` then `add_webhook(…)`, to change a subscription | `update_webhook_subscription(id, &spec)`, which replaces its options in place |

Topics are accepted as `orders/create` or `ORDERS_CREATE`.

## Schema download

| 0.10 | 1.0 |
| --- | --- |
| `download_public_admin_schema("2026-04")` | `schema::download_public_admin_schema(ApiVersion::V2026_04)` |
| `shopify.download_admin_schema()` | The same |
| `SHOPIFY_DEV_ADMIN_SCHEMA_PROXY` | `schema::PUBLIC_ADMIN_SCHEMA_ENDPOINT` |
| `ADMIN_SCHEMA_INTROSPECTION_QUERY` | `schema::INTROSPECTION_QUERY` |
| `shopify-api schema download --public --out file` | `api-version` in `[package.metadata.shopify_api]` of `Cargo.toml`: `graphql!` downloads it when cargo is asked to build the package, never for a dependency, or a packaged crate. `shopify-api schema download` refreshes it, `--introspection` for the raw result |
| `shopify-api schema download --api-version 2026-07` | No flag: the command reads the version of the nearest `Cargo.toml`. For another version, or outside a package: `schema::download_public_admin_schema(ApiVersion::new("2026-07")?)` |

`graphql!` reads the compact form it downloads, in `graphql/admin.schema.json`.

## Removed

- `utils::retry_async` and `retry_sync`: the client retries what can be.
- `utils::read_json_tree`: `serde_json::Value::pointer`.
- `utils::deserialize_from_value` and `deserialize_from_str`: errors name the path of a mismatch.
- The `debug`, `graphql-client` and `warp-wrapper` features, and the `hmac`, `sha2`, `warp` and `graphql_client` features that 0.10 published for its optional dependencies: `webhooks` and `oauth` enable what they need. The `simple_logger` dependency: logs go through `tracing`.
