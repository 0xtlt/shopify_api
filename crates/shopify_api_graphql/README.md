# shopify_api_graphql

The compile-time half of [`shopify_api`](https://crates.io/crates/shopify_api)'s
`graphql!` and `input!` macros: it reads Shopify's Admin GraphQL schema,
checks an operation against it, generates the Rust types of its variables and
of its result, and checks the input values written in Rust.

**It is an implementation detail, with no semver guarantee**: any release may
change its items. `shopify_api` depends on it with an exact `=` version.
Depend on [`shopify_api`](https://crates.io/crates/shopify_api) and use its
`graphql!` and `input!` macros instead. It is a separate crate so that the
macros and the `shopify-api` command line share it.

It bundles no schema: it reads the version a crate pins in
`[package.metadata.shopify_api]` of its `Cargo.toml`, else the one of its
schema file, downloads that version's schema (`download` feature), and stores
it in a compact form (one line per type, no descriptions).
