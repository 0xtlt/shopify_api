# shopify_api_graphql

The compile-time half of [`shopify_api`](https://crates.io/crates/shopify_api)'s
`graphql!` and `input!` macros: it reads Shopify's Admin GraphQL schema,
checks an operation against it, generates the Rust types of its variables and
of its result, and checks the input values written in Rust.

You do not need to depend on it directly: use `shopify_api::graphql!`. It is a
separate crate so that the macro and the `shopify-api` command line share it.

It bundles no schema: it reads the version a crate declares in
`[package.metadata.shopify_api]` of its `Cargo.toml` (else takes the latest
stable one), downloads that version's schema (`download` feature), and stores
it in a compact form (one line per type, no descriptions).
