# shopify_api_graphql

The compile-time half of [`shopify_api`](https://crates.io/crates/shopify_api)'s
`graphql!` macro: it reads Shopify's Admin GraphQL schema, checks an operation
against it, and generates the Rust types of its variables and of its result.

You do not need to depend on it directly: use `shopify_api::graphql!`. It is a
separate crate so that the macro and the `shopify-api` command line share it.

It also bundles the Admin schema of the API version `shopify_api` calls by
default, in a compact form (one line per type, no descriptions).
