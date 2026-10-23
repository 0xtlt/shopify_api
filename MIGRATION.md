# Upgrading from 0.10 to 1.0

1.0 keeps what 0.10 did, under new names, and adds operations checked at
compile time. Most code changes in three places: building the client, the
error type, and GraphQL calls.

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
| `ShopifyAuth::expiring_offline_token(id, secret, token)` | `Auth::expiring_token(id, secret, token)` |
| `ShopifyConfig { api_version, .. }` | `Shopify::builder(shop).api_version(ApiVersion::V2026_07)` |
| `ShopifyConfig { token_store, .. }` | `.token_store(store)` |
| `ShopifyConfig { token_refresh_leeway, .. }` | `.refresh_leeway(duration)` (a `std::time::Duration`) |
| `ShopifyConfig { user_agent, .. }` | `.user_agent(text)` |
| `ShopifyConfig { shared_secret, .. }` | Gone: pass the secret to `webhooks::Webhook::verify` |
| `ApiVersion::new("2026-07")` | The same, or `ApiVersion::V2026_07` |
| `shopify.replace_auth(auth)` | Build another client |
| `shopify.get_shop()`, `shop_domain()` | `shopify.shop()`, a `ShopDomain` |
| `shopify.get_query_url()` | `shopify.graphql_url(version)` |

`TokenStore::save_token(shop: &str, token: TokenData)` is
`TokenStore::save(shop: &ShopDomain, token: &TokenData)`, and returns a boxed
error of yours. `TokenData` has the same fields, and two more for online
tokens; build it with `TokenData::new(token)`.

The shop must now be `<name>` or `<name>.myshopify.com` (with or without
`https://`): 0.10 accepted any text and appended `.myshopify.com`.

Without a pinned version, an operation of `graphql!` goes to the version its
schema was checked against (the `api-version` of your `Cargo.toml`, else the
latest), and an untyped call to 2026-10. 0.10 sent
everything to 2026-04 by default: pin it with `.api_version(ApiVersion::V2026_04)`
to keep that.

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
| `MissingGraphqlData`, `InvalidHeaderValue`, `Other` | Gone |

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

`object_count`, `root_object_count` and `file_size` are numbers.

## Webhooks

The `webhooks` feature verifies and reads webhooks without a web framework, so
the `warp-wrapper` feature, `Shopify::warp_wrapper` and the `ShopifyWebhook`
payload structs are gone. In your handler, with the raw body:

```rust,ignore
let webhook = shopify_api::webhooks::Webhook::verify(headers, body, client_secret)?;
let order: MyOrder = webhook.payload()?;
```

| 0.10 | 1.0 |
| --- | --- |
| `shopify.verify_hmac(data, header)` | `webhooks::verify_hmac(secret, data, header)` |
| `list_webhooks()` | `webhook_subscriptions()`, every page |
| `add_webhook(address, topic, format)` | `create_webhook_subscription(&WebhookSubscriptionSpec::new(topic, address))` |
| `delete_webhook(id)` | `delete_webhook_subscription(id)` |
| `webhook_auto_config(vec![(address, topic, format)])` | `sync_webhook_subscriptions(&[WebhookSubscriptionSpec::new(topic, address)])`, which returns what it did |
| `edit_webhook` | Delete and create, or `sync_webhook_subscriptions` |

Topics are accepted as `orders/create` or `ORDERS_CREATE`.

## Schema download

| 0.10 | 1.0 |
| --- | --- |
| `download_public_admin_schema("2026-04")` | `schema::download_public_admin_schema(ApiVersion::V2026_04)` |
| `shopify.download_admin_schema()` | The same |
| `shopify-api schema download --public --out file` | `api-version` in `[package.metadata.shopify_api]` of `Cargo.toml`: `graphql!` downloads it. `shopify-api schema download` refreshes it, `--introspection` for the raw result |

`graphql!` reads the compact form it downloads, in `graphql/admin.schema.json`.

## Removed

- `utils::retry_async` and `retry_sync`: the client retries what can be.
- `utils::read_json_tree`: `serde_json::Value::pointer`.
- `utils::deserialize_from_value` and `deserialize_from_str`: errors name the path of a mismatch.
- The `debug` and `graphql-client` features, and the `simple_logger` dependency: logs go through `tracing`.

# Upgrading from a 1.0 preview

The input objects of `graphql!` changed before the release.

| 1.0 preview | 1.0 |
| --- | --- |
| `module::MetafieldsSetInput { owner_id: id.to_string(), namespace: Some("custom".into()), ..Default::default() }` | `input!(MetafieldsSetInput { ownerId: id, namespace: "custom", … })`: GraphQL field names, every required field, `?:` for an `Option` |
| `module::MetafieldsSetInput` as a struct | The same name, a type alias of the `InputObject` every `input!(MetafieldsSetInput { … })` builds |
| An inline input object variable given `json!({ … })` or any `Serialize` value | An object literal, `product = { id: id, … }`, or values of `input!`; `json!` goes through `Shopify::graphql` |
| An enum variable given a string, `sortKey = "TITLE"` or `sortKey = key` | `sortKey = TITLE`, checked, or `sortKey = raw(key)` |
| No way to send `null` for an input field | `field: null`, or `field?: Maybe::Null` |
| One module per `graphql! { … }` | Several, `;` between them |
| `shopify.execute(module::operation(module::Variables { … }))` | The same, or `shopify.execute(module::Variables { … })` |
| `Variables` always `Default` and `PartialEq` | `Default` when no variable is required, `PartialEq` when it holds no input object |
| `Operation::body()` returned a `serde_json::Value` | It returns the exact JSON text `execute` sends, a `String`; parse it yourself for a `Value`, `serde_json::from_str::<Value>(&body)` |
| `Operation::variables_json()` returned `&Map` | `Operation::variable_json(name)` returns the JSON text of one variable, `Option<&str>`; nothing is parsed back |
| `shopify.execute(module::Variables { … }.into())` | Drop the `.into()`: `execute` takes `impl Into<Operation<D>>`, so `.into()` has no single target (E0283) |
| `Operation::variable::<T, _>` with a `T` of yours | `T` is a `VariableType`: one of the Rust types `graphql!` picks (`String`, `i64`, `f64`, `bool`, `Value`, `InputObject`, in `Option` and `Vec`). `json_variable` takes anything else |
| A `u64` beyond `i64` saturated to `i64::MAX`; NaN and infinity sent as `null`; `null` sent for a non-null `JSON` | Refused: `Error::InvalidValue` when the operation runs, before the request; an integer literal out of the range of `Int` is a compile error |
| `[for _ in … => …]` | `[for _item in … => …]`: the pattern binds a name |
