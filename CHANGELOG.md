# Changelog

## 1.0.0

A rewrite around operations checked at compile time. See [MIGRATION.md](MIGRATION.md).

- Add: `graphql!`, which checks a query or mutation against Shopify's Admin schema while the crate compiles and generates the types of its variables and result. Inline (`graphql!("…", first = 10)`), named (`graphql! { pub mod name = "…"; }`, with typed input objects) or read from a file (`file("…graphql")`).
- Add: deprecated fields, arguments, input fields and enum values are compiler warnings with Shopify's reason.
- Add: `graphql!` refuses a nullable variable whose default is `null` where a non-null value is expected.
- Add: no schema ships with the crates. `graphql!` downloads the schema of the crate's version into `graphql/admin.schema.json`: the latest stable version, or the one `api-version` in `[package.metadata.shopify_api]` of `Cargo.toml` pins. It downloads again when the version changes, and operations are sent to the version they were checked against. `shopify-api schema download` refreshes the file, `--only` cuts it down to some operations.
- Add: `shopify-api check` checks `.graphql` files against the schema.
- Add: retries: a throttled request waits for what its cost needs, a `429` for its `Retry-After`, in seconds or as an HTTP date (up to `RetryPolicy::max_retry_after`, five minutes by default, else `Error::Throttled` with `retry_after`); queries are retried after a `5xx` or a network failure, mutations never, nor any document that cannot be read as queries only. `RetryPolicy`.
- Add: tokens are renewed once for all the clones of a client, before they expire and when Shopify refuses one with a `401`. `Auth::expiring_token` with a `TokenStore`.
- Add: `Shopify::paginate`, `Page`.
- Add: bulk operations: `bulk_query_rows`, `run_bulk_mutation` (uploads the JSONL), `cancel_bulk_operation`, typed `BulkOperation` and statuses, adaptive polling.
- Add: `webhooks`: `Webhook::verify` (headers, signature with secret rotation, payload), subscriptions with `sync_webhook_subscriptions`, which updates in place and creates before it deletes.
- Add: `oauth`: `ShopifyApp` with install links, callback and admin launch verification, code exchange, session tokens and token exchange, app proxies.
- Add: `Transport` trait, `ReqwestTransport` (redirections always off, `from_builder` for your own settings), and `testing::MockTransport`, with `testing::client`, which retries without waiting, even for a scripted `Retry-After`.
- Add: token renewals run in a task of their own: a cancelled request does not lose a rotated refresh token. A failed renewal answers the requests waiting for it.
- Add: errors never hold a query string (signed bulk result addresses stay out of logs), and the `Debug` of `GraphqlErrors` names the fields of partial `data` without their values; `Error::Http` carries Shopify's `X-Request-Id`.
- Add: `ShopDomain` refuses anything but `<name>` or `<name>.myshopify.com`.
- Breaking: `Shopify::new(shop, auth)` and `Shopify::builder`; `ShopifyAuth` is `Auth`; `ShopifyConfig` is gone; `ShopifyAPIError` is `Error`.
- Breaking: `graphql` returns `data` as a `Value` and `graphql_as` reads it as a type; `post_graphql` and the `graphql-client` feature are gone.
- Breaking: the warp wrapper, the webhook payload structs, `utils` (`retry_async`, `read_json_tree`) and `graphql_at_path` are gone.
- Breaking: Rust 1.88, edition 2024.

## 0.10.1

- Security: refresh `Cargo.lock` to pick up fixes for RUSTSEC-2026-0185, -0285, -0338, -0339, -0340, -0342, -0346, -0348, -0349, -0382 and -0383 (`quinn-proto`, `rustls`, `rustls-webpki`, `webpki-root-certs`).
- Updated: raise minimum versions of `reqwest` (0.13.5), `thiserror` (2.0.21), `bytes` (1.12) and `clap` (4.6.7).

## 0.10.0

- Breaking: refactor the client around Shopify Admin GraphQL API `2026-04` and newer.
- Breaking: remove the public REST module and built-in Shopify schema JSON files.
- Add: `ShopifyAuth` for static access tokens, client credentials, and expiring offline token refresh.
- Add: structured GraphQL responses through `graphql_raw` and typed data extraction through `graphql`.
- Add: dynamic Admin schema download at runtime and through the `shopify-api` CLI behind the `cli` feature.
- Add: public Admin schema download through Shopify's `admin-graphql-direct-proxy` endpoint.
- Updated: bulk operations now use `bulkOperation(id:)`, `bulkOperations`, `text/jsonl` staged uploads, and configurable polling.

## 0.9.6

- Security: update `openssl` to `0.10.79` in `Cargo.lock` to resolve Dependabot alerts affecting the optional `native-tls` feature path.

## 0.9.5

- Add: `debug` feature added, enabled by default

## 0.9.1

- Fixed: `OrderLineItem` type fixed

## 0.9.0

- Fixed: `ShopifyWebhook::OrdersUpdated` type fixed

## 0.8.10

- Fixed: `OrderCustomer` & `Order` struct fixed

## 0.8.9

- Added: `ShopifyWebhook::OrdersUpdate` type added

## 0.8.8

- Fixed: `DateTime` type added

## 0.8.7

- Fixed: `wait_for_bulk` method condition

## 0.8.5

- Fixed: `wait_for_bulk` method condition

## 0.8.4

- Fixed: `UnsignedInt64` types to String

## 0.8.3

- Fixed: `confirmation_number` in `order` struct

## 0.8.2

- Fixed: Webhook Customer Struct

## 0.7.0

- Add: `graphql_client` feature
- Add: `post_graphql` method to `Shopify` with `graphql_client` feature on
- Edited: `warp_wrapper` method takes one more argument

## 0.6.0

- Add: `verify_hmac` method added to `Shopify`
- Add: `ShopifyWebhook` struct
- Add: `list_webhooks` method added to `Shopify`
- Add: `add_webhook` method added to `Shopify`
- Add: `edit_webhook` method added to `Shopify`
- Add: `delete_webhook` method added to `Shopify`
- Add: `webhook_auto_config` method added to `Shopify`

## 0.5.0

- Add: `stage_upload_json` method added to `Shopify`
- Add: `generate_staged_upload_url` method added to `Shopify`
- Add: `stage_upload_prepare` method added to `Shopify`
- Removed: `ShopifyAPIVersion` enum replaced by a string

## 0.4.7

- Fixed: `get_end_of_support_date` enum by @davidhollenbeckx

## 0.4.5

- Add: `download_bulk` method added to `Shopify`

## 0.4.2

- Add: The body response (string) to `NotWantedJsonFormat` error

## 0.4.1

- Add: `wait_for_bulk` method added to `Shopify`

## 0.4.0

- Add: `make_bulk_query` method added to `Shopify`
- Add: `get_bulk_by_id` method added to `Shopify`

## 0.3.6

- Updated: `ShopifyAPIVersion` enum

## 0.3.5

- Updated: `ShopifyAPIVersion` enum to add the latest version `V2023_04`

## 0.3.0

- Add the support of the REST API with the `rest_query` method added to `Shopify`
- Add `get_api_endpoint` method to `Shopify`
- Add `rest_url` method to `Shopify`
- Fix a bug for `retry_async` and `retry_sync` functions which run 2 times at least the given function.

## 0.2.1

- Add Changelog and readme
