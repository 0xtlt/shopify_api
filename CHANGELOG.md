# Changelog

## 1.0.0 (Unreleased)

A rewrite around operations checked at compile time. See [MIGRATION.md](MIGRATION.md).

### Added

- `graphql!`, which checks a query or mutation against Shopify's Admin schema while the crate compiles and generates the types of its variables and result. Inline (`graphql!("…", first = 10)`), named (`graphql! { pub mod name = "…"; }`) or read from a file (`file("…graphql")`).
- Input values written as literals in an inline `graphql!`, checked against the schema at the token: `product = { id: id, status: ACTIVE, descriptionHtml: null, vendor?: vendor, seo: { title: t }, metafields: [for m in &rows if cond => { … }] }`. Field names are the GraphQL ones (`type` included); an unknown or repeated field, a missing required field (each one named), `null` on a non-null type, an unknown enum value (the valid ones listed) and a value of the wrong form are compile errors; deprecated fields and enum values warn. `field?: value` leaves the field out for `None`; an `Option` given to `field: value` does not compile, since `null` would clear the field. `raw(string)` sends an enum value unchecked. Rust expressions are checked by traits whose errors name the GraphQL type. Literals are written straight to JSON, with no `serde_json::Value` in between.
- `input!(TypeName { … })` builds an input object ahead of the call, with the same checks, as an `InputObject`: one Rust type per GraphQL name, the same in every module and crate, so that the values of several `input!` go in one `Vec`.
- `Maybe<T>` (`Absent`, `Null`, `Value`), for `field?:` when `null` is a choice made at run time.
- Values Shopify would not read as written are refused. A `Float` must be finite (NaN and infinity were sent as `null`), an `Int` must fit in 32 bits, a non-null `JSON` must not be `null` at its top. An integer literal out of range, alone, negated, in parentheses or in a list `[…]`, is a compile error at the literal; `Some(…)`, `vec![…]` and other forms are checked at run time. A value known at run time, in a literal, an `input!`, a module's `Variables` or `Operation::variable`, is kept as refused, and the operation fails with `Error::InvalidValue { path, reason, detail }` before anything is sent: `` invalid value at `$product.metafields[2].value`: not a finite Float ``. The message names the place and the reason (`InvalidValueReason`), never the value; a serializer's message is in `sensitive_detail()`. An `InputObject` that is refused fails to serialize on its own. Nothing in the code of the macros panics on a value.
- No name the macros generate is in scope where the values are evaluated: a local, closure, function or constant of yours with the name of a generated item stays yours.
- One `graphql! { … }` holds several modules, `;` between them. A module's `Variables` runs as it is, `shopify.execute(module::Variables { … })`, and it names each input object its variables may hold: `pub type MetafieldsSetInput = InputObject<…>`.
- Deprecated fields, arguments, input fields and enum values are compiler warnings with Shopify's reason.
- `graphql!` refuses a nullable variable whose default is `null` where a non-null value is expected, an input field given twice in one input object, and one key selected twice with arguments that differ in kind (`1` and `1.0`, `"A"` and `A`) or in the order of an input object's fields, which Shopify tells apart. One response key may name different fields on two different object types, and below them, when the shapes of the answers agree (SameResponseShape), even fields of different object types: their subfields merge into one struct, each optional unless both sides select it. A field of an object type selected without subfields at one of its places is refused. So is a block string whose lines are indented with different Unicode whitespace (U+00A0 and U+2003), on which the GraphQL parser would panic.
- `Operation`, what `graphql!` builds, or `Operation::new` for any text: `execute` and `execute_response` take `impl Into<Operation<D>>`, a module's `Variables` included. `body()` is the exact JSON text sent, and `variable_json(name)` the text of one variable; `with_name` sets the `operationName`.
- No schema ships with the crates. Each crate that uses `graphql!` keeps the schema of its Admin API version in `graphql/admin.schema.json`, and operations are sent to the version they were checked against. The version is `api-version` in `[package.metadata.shopify_api]` of `Cargo.toml`, else the one of the schema file, else (first build, no file) the latest stable one: a new Shopify release never changes a build. A local package (of the workspace, or a path dependency) that cargo was asked to build downloads the file when it is missing or does not hold the pinned version, once per build; any other crate never downloads nor writes it, and fails to build when its file is missing or does not fit, a packaged crate in the verification of `cargo package` included. A packaged, vendored or checked-out crate (a `Cargo.toml.orig`, a `.cargo-checksum.json` or a `.cargo-ok` in the package directory, a `.cargo-ok` above it, or a path under `$CARGO_HOME`, with or without symbolic links resolved) never downloads: where a crate is decides only between a download and a compile error, never which schema it gets. `shopify-api schema download` refreshes the file, `--only` cuts it down to some operations.
- `schema = "<path>"` in `[package.metadata.shopify_api]` names another schema file than `graphql/admin.schema.json`, relative to the package's `Cargo.toml`, for that package only. It must name a file inside the package, which `cargo package` ships: a relative path with `/` and portable names (no `:`, `\`, `<>"|?*` or control character, no name ending in `.` or a space, no device name such as `CON`); an absolute path, or one that climbs out with `..`, is an error (a symbolic link inside the package may point elsewhere). A dependency keeps the one its own `Cargo.toml` names. The file follows the rules of the default one (version, download, older format), `shopify-api schema download` and `shopify-api check` use it, and an edit of the key compiles the crate again.
- `@oneOf` input objects (`ProductSetIdentifiers`…) are checked at compile time: exactly one field, not `null`.
- `shopify-api check` checks `.graphql` files against the schema.
- Retries, on by default (four): a throttled request waits for what its cost needs, a `429` for its `Retry-After`, in seconds or as an HTTP date (up to `RetryPolicy::max_retry_after`, five minutes by default, else `Error::Throttled` with `retry_after`); queries are retried after a `5xx` or a network failure, mutations never, nor any document that cannot be read as queries only. `RetryPolicy`, and `RetryPolicy::none()` for none. `RetryPolicy` and `BulkWaitOptions` are `Clone` and `Debug`, not `Copy` nor `PartialEq`, so that they may later hold a callback. The jitter of the backoff is random on every platform.
- `Shopify::paginate`, `Page`.
- Bulk results read as they download, in memory that follows the line being read rather than the file: `bulk_query_stream` and `download_bulk_results_stream` return `BulkRows<T>`, a `Stream` with `next_row`. Lines may span chunks, end in `\r\n` or have no final newline; blank lines are skipped. A line that does not decode is an `Error::Decode` with its line number, and the stream goes on; a line longer than `max_line_length` (`DEFAULT_MAX_LINE_LENGTH`, 64 MiB) or a failed download ends it. `bulk_query_rows` and `download_bulk_results` collect the same stream.
- `bulk_query_grouped` (runs with `groupObjects`) and `download_bulk_results_grouped` return `BulkGroups<P, C>`, each top-level object with the objects of its nested connections, one group in memory at a time. A child out of its group is an error, not a child in the wrong group; so is a child whose parent has no unaliased `id`, which the error names.
- `run_bulk_mutation_iter` and `stage_bulk_variables_iter` take the variables from an iterator; the JSONL is written straight into the body of the upload, in memory once.
- Bulk operations checked at compile time: `graphql!(bulk("…"))`, `bulk(file("…"))`, inline or in a module. A bulk query is checked against Shopify's bulk rules for API 2026-10: one top-level field, a connection (not `node` or `nodes`); five connections at most, two levels deep below it; `edges { node { … } }` and nothing else (no `nodes`, `cursor`, `pageInfo`, nor `first`, `last`, `after`, `before`); `Node` objects; a nested connection written right in the `node` of its parent; an unaliased `id` on every node with nested connections (an error that asks for it, never added silently), and no other field aliased `id` there; no alias `_shopifyApiTypename` or starting with `__`; no string that the parser reads unlike GraphQL (a `\b` escape, which it reads as U+0010: write `\u0008`; a block string with a lone carriage return or indented with other whitespace than spaces and tabs); two connections of one node that may return the same type, such as two aliased `metafields`, are refused. What the guide does not show is refused. A bulk mutation is one mutation field, not a bulk operation, with one connection at most.
- The items of a bulk query are generated types: `Item`, each nested connection a `Vec` of its nodes. `Shopify::bulk_query` runs it with `groupObjects` and returns `BulkItems<Item>` (`next_item`, `collect`, `Stream`), one item in memory at a time, nested lines placed by `__parentId`, and by `__typename` where a node has several connections: the text sent selects it there as `_shopifyApiTypename: __typename`, and a `__typename` of the query is left as written. A line's `id` places its children only when it is a string: `id` may alias another field where no connection is nested. `start_bulk_query` and `download_bulk_items` split the steps; `BulkQuery::text` is the text sent.
- The variables of a bulk query, which Shopify does not take, are given as for any operation (literals and `input!` included) and written into its text as GraphQL literals, from the parsed operation: strings escaped as JSON, enum values bare (in input objects and lists too), input objects with unquoted keys, a variable without a value (a `None` in `Variables`) left out or given its default, an explicit `null` written `null`. The values are checked as for any operation; an enum value or a field that is not a GraphQL name fails with `InvalidValueReason::NotAGraphqlName`, an enum value `true`, `false` or `null` with `InvalidValueReason::ReservedEnumValue`. A variable that may hold `JSON` is refused at compile time.
- `Shopify::bulk_mutation` runs a `graphql!(bulk("mutation …"))` over an iterator of its `Variables`: every line is checked and written before anything is sent, then staged, run, and read as `BulkMutationResult<Data>` (`data`, `errors`, `index`, Shopify's `__lineNumber`). `start_bulk_mutation` only starts it. Typed bulk operations run at the Admin API version they were checked against.
- `Transport`, the HTTP layer as a trait, with `send_streaming` (a default goes through `send`), `StreamingResponse` and `BodyStream`. The default `ReqwestTransport` never follows a redirection, and streams: the timeout of a request bounds the wait for the headers and for each chunk, not the whole body. A client of your own goes through `transport::reqwest_0_13::from_builder`: reqwest appears in the API only under `transport::reqwest_0_13`, named after its major version, so that a later reqwest comes next to it. The client runs on Tokio.
- `testing::MockTransport` (`push_data`, `push_chunks`, `push_streaming`, `respond_with`…), and `testing::client`, which retries without waiting, even for a scripted `Retry-After`. `testing` also gives the signing helpers: `webhooks::sign`, `ShopifyApp::sign_query`, `sign_app_proxy`, `sign_session_token`.
- `oauth`: `ShopifyApp`, with install links, the verification of callbacks, admin launches and app proxy requests, the code exchange, session tokens and the token exchange. `verify_session_token` takes only the session tokens of Shopify admin: `dest` exactly `https://{shop}.myshopify.com` and `iss` exactly its `/admin`, compared whole; `SessionToken::user_id` is the staff member's ID. `verify_extension_session_token` takes the tokens of checkout and customer account UI extensions, which any buyer gets (a `dest` with or without `https://`, `iss` optional but then the admin's), as an `ExtensionSessionToken` with `shop()`, `customer()` (the customer's GID) and `claims()`: a type of its own, which does not exchange for a token, so that a buyer's token never passes for an admin session. `ShopifyApp::with_token_store` gives the clients it builds a `TokenStore`.
- An error shows nothing of what the crate did not write (`Error` documents the contract): no body, value or map key of an answer, no message of serde or of a `Serialize` implementation, no query string or user of an address. `Error::Http` displays the status, the address, the content type and the length of the body; `Error::raw_http_body` (or the `raw_body: RawBody` field) gives the body. `Error::Decode` displays the kind of mismatch, its line and column, and a path of the type's own field names and indexes, with `[key]` for a map key. `Error::Variables` names the variable that does not serialize. `Error::sensitive_detail` gives serde's message, the `Serialize` implementation's or the token endpoint's, all kept out of `Display` and `Debug` (`SensitiveDetail`). The `Debug` of `GraphqlErrors` counts the entries of partial `data`, and that of `GraphqlError` shows `extensions.code` and counts the other extensions (Shopify echoes an invalid variable's whole value in `extensions.value`). Shopify's own messages in `errors` and `userErrors` are shown as they are. `Error::Http` carries Shopify's `X-Request-Id`.
- `Error::Bulk { message, operation }` holds the operation that ended without completing, with its `error_code` and `partial_data_url`; `Error::Timeout { message, id }` the ID of the bulk operation waited for, to cancel it.

### Changed

- Tokens: 0.10 renewed a token before it expired and saved it with a `TokenStore`. 1.0 also renews when Shopify refuses one with a `401`; one renewal serves every clone of a client and the requests waiting for it; it runs in a task of its own, so a cancelled request does not lose a rotated refresh token; a failed renewal answers the requests waiting for it. Until a renewed token is saved, nothing is sent: each call tries a failed save again first, and fails with `Error::TokenStore`. A refresh answer without a refresh token keeps the current one. `Auth::with_token` starts from a stored token.
- Bulk operations: 0.10 ran, read, listed and waited for them, with typed statuses. 1.0 adds `bulk_query_rows` (the whole cycle), `run_bulk_mutation` (uploads the JSONL), `cancel_bulk_operation`, and polling that starts at one second and backs off. `BulkOperation` has numbers for its counts and `DateTime<Utc>` for its dates; `BulkOperationStatus` and `BulkOperationType` serialize as Shopify writes them (`"COMPLETED"`, an unknown one as its text) and round-trip.
- Webhook subscriptions: 0.10 listed, added, deleted and auto-configured them. 1.0 has `webhook_subscriptions` (every page), `create_`, `update_` (in place, new) and `delete_webhook_subscription`, and `sync_webhook_subscriptions`, which updates in place, creates before it deletes, and returns what it did. `WebhookSubscription` dates are `DateTime<Utc>`. Subscriptions are JSON only: `WebhookSubscriptionSpec` has no format, and `sync_webhook_subscriptions` updates an XML subscription of a desired topic and address to JSON.
- Webhook verification: 0.10 checked the HMAC. `Webhook::verify` also reads the headers (topic, shop, IDs; with or without `X-`, in any case, from any `(name, value)` pairs, `&http::HeaderMap` included), accepts several secrets during a rotation, and verifies JSON webhooks only, refusing a body whose first byte is not `{` (whitespace and a byte order mark before it included): the launch queries, session tokens and app proxy requests the client secret also signs, which an app proxy query can make read as XML, or as JSON after a tab, can never pass for a webhook. `Webhook::verify_payload_shop` checks that a destructive webhook names, in its payload, the unsigned shop of its headers. `Debug` of a `Webhook` shows the length of its body, not the body.
- Schema download: `schema::download_public_admin_schema` and `download_admin_schema` take an `ApiVersion`.
- `ShopDomain` refuses anything but `<name>` or `<name>.myshopify.com`, `http://` included; 0.10 appended `.myshopify.com` to any text.
- Without a pinned version, an operation of `graphql!` goes to the version of its schema, and an untyped call to `ApiVersion::DEFAULT`, 2026-10 (0.10: 2026-04). The crate's own operations go to a pinned version without a warning.

### Breaking

- `Shopify::new(shop, auth)` and `Shopify::builder`; `ShopifyAuth` is `Auth`; `ShopifyConfig` is gone; `replace_auth`, `token_url`, the public `api_version` field, `request_client_credentials_token` and `refresh_token_with_credentials` are gone (the client asks for its tokens itself). `DEFAULT_API_VERSION`, `MIN_API_VERSION` and `VERSION` are `ApiVersion::DEFAULT`, `ApiVersion::MINIMUM` and `USER_AGENT`; `ApiVersion::new` takes a `&str`.
- `ShopifyAPIError` is `Error`, which no longer converts from `reqwest::Error` or `serde_json::Error`. `MissingGraphqlData` is gone: an answer without `data` gives `Value::Null`.
- `TokenStore::save_token(&str, TokenData)` is `save(&ShopDomain, &TokenData)`; `TokenData` is `#[non_exhaustive]`, built with `TokenData::new`; `expires_within` takes a `std::time::Duration`.
- `graphql` returns `data` as a `Value` and `graphql_as` reads it as a type; `post_graphql` and the `graphql-client` feature are gone; the untyped calls send no `operationName`.
- `GraphqlResponse` is `RawResponse`; the `shopify_api::graphql` module is private (`graphql` is the macro); `parse_jsonl` takes bytes; the staged upload and connection types are internal.
- The warp wrapper, the webhook payload structs, `utils` (`retry_async`, `read_json_tree`) and `graphql_at_path` are gone, as are `run_bulk_queries`, `run_bulk_mutations` and `BulkConcurrencyOptions`, and the features `debug`, `graphql-client`, `warp-wrapper`, `hmac`, `sha2`, `warp` and `graphql_client`.
- Rust 1.88, edition 2024.

### Packaging

- The three crates are released together, each requiring the exact version of the others, and published at once (`cargo publish --workspace`). Each package includes the license. `cargo test` of the published `shopify_api` runs offline.
- Security: `Cargo.lock` picks up `ctutils` 0.4.6 for RUSTSEC-2026-0385. chrono 0.4.39 or newer.

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
