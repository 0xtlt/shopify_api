# Changelog

## 1.0.0

A rewrite around operations checked at compile time. See [MIGRATION.md](MIGRATION.md).

- Add: `graphql!`, which checks a query or mutation against Shopify's Admin schema while the crate compiles and generates the types of its variables and result. Inline (`graphql!("…", first = 10)`), named (`graphql! { pub mod name = "…"; }`) or read from a file (`file("…graphql")`).
- Add: input values written as literals in an inline `graphql!`, checked against the schema at the token: `product = { id: id, status: ACTIVE, descriptionHtml: null, vendor?: vendor, seo: { title: t }, metafields: [for m in &rows if cond => { … }] }`. Field names are the GraphQL ones (`type` included); an unknown or repeated field, a missing required field (each one named), `null` on a non-null type, an unknown enum value (the valid ones listed) and a value of the wrong form are compile errors; deprecated fields and enum values warn. `field?: value` leaves the field out for `None`; an `Option` given to `field: value` does not compile, since `null` would clear the field. `raw(string)` sends an enum value unchecked. Rust expressions are checked by traits whose errors name the GraphQL type. Literals are written straight to JSON, with no `serde_json::Value` in between.
- Add: `input!(TypeName { … })` builds an input object ahead of the call, with the same checks, as an `InputObject`: one Rust type per GraphQL name, the same in every module and crate, so that the values of several `input!` go in one `Vec`.
- Add: `Maybe<T>` (`Absent`, `Null`, `Value`), for `field?:` when `null` is a choice made at run time.
- Add: values Shopify would not read as written are refused. A `Float` must be finite (NaN and infinity were sent as `null`), an `Int` must fit in 32 bits, a non-null `JSON` must not be `null` at its top. An integer literal out of range is a compile error at the literal. A value known at run time, in a literal, an `input!`, a module's `Variables` or `Operation::variable`, is kept as refused, and the operation fails with `Error::InvalidValue { path, reason, detail }` before anything is sent: `` invalid value at `$product.metafields[2].value`: not a finite Float ``. The message names the place and the reason (`InvalidValueReason`), never the value; a serializer's message is in `sensitive_detail()`. An `InputObject` that is refused fails to serialize on its own. Nothing in the code of the macros panics on a value.
- Add: no name the macros generate is in scope where the values are evaluated: a local, closure, function or constant of yours with the name of a generated item stays yours.
- Add: one `graphql! { … }` holds several modules, `;` between them. A module's `Variables` runs as it is, `shopify.execute(module::Variables { … })`, and it names each input object its variables may hold: `pub type MetafieldsSetInput = InputObject<…>`.
- Breaking (against the 1.0 previews): the input structs of modules (`MetafieldsSetInput { owner_id, ..Default::default() }`) are gone, for `input!`, which cannot leave out a required field. `Variables` implements `Default` only when no variable is required, and `PartialEq` only when it holds no input object. An input object variable of an inline `graphql!` takes a literal or values of `input!`, no longer any `Serialize` value such as `json!`. An enum variable takes a bare value (`sortKey = TITLE`) or `raw(string)`, no longer a string. `execute` and `execute_response` take `impl Into<Operation<D>>`, so `execute(variables.into())` no longer infers its type: drop the `.into()`. `Operation` keeps each variable as the JSON it is sent as: `variables_json` returns the variables read back, by value, as deep as they were written. `Operation::variable::<T, _>` takes a `T` of the sealed `VariableType`, and a `u64` beyond the range of `Int` is refused rather than saturated. The pattern of `[for … => …]` binds a name.
- Add: deprecated fields, arguments, input fields and enum values are compiler warnings with Shopify's reason.
- Add: `graphql!` refuses a nullable variable whose default is `null` where a non-null value is expected, an input field given twice in one input object, and one key selected twice with arguments that differ in kind (`1` and `1.0`, `"A"` and `A`) or in the order of an input object's fields, which Shopify tells apart. One response key may name different fields on two different object types, and below them, when the shapes of the answers agree (SameResponseShape), even fields of different object types: their subfields merge into one struct, each optional unless both sides select it. A field of an object type selected without subfields at one of its places is refused.
- Add: no schema ships with the crates. `graphql!` downloads the schema of the crate's version into `graphql/admin.schema.json`: the latest stable version, or the one `api-version` in `[package.metadata.shopify_api]` of `Cargo.toml` pins. It downloads again when the version changes, and operations are sent to the version they were checked against. `shopify-api schema download` refreshes the file, `--only` cuts it down to some operations.
- Add: `shopify-api check` checks `.graphql` files against the schema.
- Add: retries: a throttled request waits for what its cost needs, a `429` for its `Retry-After`, in seconds or as an HTTP date (up to `RetryPolicy::max_retry_after`, five minutes by default, else `Error::Throttled` with `retry_after`); queries are retried after a `5xx` or a network failure, mutations never, nor any document that cannot be read as queries only. `RetryPolicy`.
- Add: tokens are renewed once for all the clones of a client, before they expire and when Shopify refuses one with a `401`. `Auth::expiring_token` with a `TokenStore`.
- Add: `Shopify::paginate`, `Page`.
- Add: bulk operations: `bulk_query_rows`, `run_bulk_mutation` (uploads the JSONL), `cancel_bulk_operation`, typed `BulkOperation` and statuses, adaptive polling.
- Add: bulk results read as they download, in memory that follows the line being read rather than the file: `bulk_query_stream` and `download_bulk_results_stream` return `BulkRows<T>`, a `Stream` with `next_row`. Lines may span chunks, end in `\r\n` or have no final newline; blank lines are skipped. A line that does not decode is an `Error::Decode` with its line number, and the stream goes on; a line longer than `max_line_length` (`DEFAULT_MAX_LINE_LENGTH`, 64 MiB) or a failed download ends it. `bulk_query_rows` and `download_bulk_results` collect the same stream.
- Add: `bulk_query_grouped` (runs with `groupObjects`) and `download_bulk_results_grouped` return `BulkGroups<P, C>`, each top-level object with the objects of its nested connections, one group in memory at a time. A child out of its group is an error, not a child in the wrong group; so is a child whose parent has no unaliased `id`, which the error names.
- Add: `run_bulk_mutation_iter` and `stage_bulk_variables_iter` take the variables from an iterator; the JSONL is written straight into the body of the upload, in memory once.
- Add: `Transport::send_streaming`, with a default that goes through `send`, and `StreamingResponse`, `BodyStream`. `ReqwestTransport` streams: the timeout of the request bounds the wait for the headers and for each chunk, not the whole body. `MockTransport::push_chunks` and `push_streaming` script a body in chunks.
- Add: `webhooks`: `Webhook::verify` (headers, signature with secret rotation, payload), subscriptions with `sync_webhook_subscriptions`, which updates in place and creates before it deletes.
- Add: `oauth`: `ShopifyApp` with install links, callback and admin launch verification, code exchange, session tokens and token exchange, app proxies.
- Add: `Transport` trait, `ReqwestTransport` (redirections always off, `from_builder` for your own settings), and `testing::MockTransport`, with `testing::client`, which retries without waiting, even for a scripted `Retry-After`.
- Add: token renewals run in a task of their own: a cancelled request does not lose a rotated refresh token. A failed renewal answers the requests waiting for it.
- Add: an error shows nothing of what the crate did not write (`Error` documents the contract): no body, value or map key of an answer, no message of serde or of a `Serialize` implementation, no query string or user of an address. `Error::Http` displays the status, the address, the content type and the length of the body; `Error::raw_http_body` (or the `raw_body: RawBody` field) gives the body. `Error::Decode` displays the kind of mismatch, its line and column, and a path of the type's own field names and indexes, with `[key]` for a map key. `Error::Variables` names the variable that does not serialize. `Error::sensitive_detail` gives serde's message, the `Serialize` implementation's or the token endpoint's, all kept out of `Display` and `Debug` (`SensitiveDetail`). The `Debug` of `GraphqlErrors` counts the entries of partial `data`, and that of `GraphqlError` shows `extensions.code` and counts the other extensions (Shopify echoes an invalid variable's whole value in `extensions.value`). Shopify's own messages in `errors` and `userErrors` are shown as they are. `Error::Http` carries Shopify's `X-Request-Id`.
- Breaking (against the 1.0 previews): `Error::Http` displays no part of the body and has no `body` field any more (it was a redacted 1 KiB preview); it has `content_type`. `Error::Json` is gone: a body that is not JSON is `Error::Decode`. `Error::Decode` has a `detail` field and its path writes a map key `[key]`. `Error::Variables(String)` and `Error::Auth(String)` are `Variables { message, detail }` and `Auth { message, detail }`. `Error` no longer converts from `serde_json::Error`. `InvalidShop` and `InvalidApiVersion` do not display the text given, which may be a token given in the wrong place (`InvalidApiVersion` shows a version written `YYYY-MM`).
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
