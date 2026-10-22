//! Compile-time support for the typed Shopify Admin GraphQL operations of
//! [`shopify_api`](https://docs.rs/shopify_api).
//!
//! [`analyze`](fn@analyze) checks an operation against a [`Schema`] and describes the
//! shape of its variables and of its result; [`codegen`] turns that
//! description into Rust types. The `graphql!` macro is a thin layer over
//! both, and the `shopify-api schema download` command uses
//! [`Schema::from_introspection`] and [`Schema::to_json`] to store the schema
//! of another API version.
#![forbid(unsafe_code)]

pub mod analyze;
pub mod codegen;
pub mod schema;

pub use analyze::{
    Deprecation, Error, FieldBase, Operation, OperationKind, Scalar, SelectedField, Selection,
    Variable, VariableBase, analyze,
};
pub use schema::{
    BUNDLED_API_VERSION, Field, INTROSPECTION_QUERY, InputValue, Schema, SchemaError, TypeDef,
    TypeRef,
};

/// The oldest Admin API version `shopify_api` calls.
pub const MINIMUM_API_VERSION: &str = "2026-04";

/// Whether `version` is an Admin API version `shopify_api` calls: a quarterly
/// release such as `2026-10`, from [`MINIMUM_API_VERSION`] on, or `unstable`.
pub fn is_supported_api_version(version: &str) -> bool {
    if version == "unstable" {
        return true;
    }
    let bytes = version.as_bytes();
    let well_formed = bytes.len() == 7
        && bytes[4] == b'-'
        && bytes[..4].iter().chain(&bytes[5..]).all(u8::is_ascii_digit)
        && matches!(&version[5..], "01" | "04" | "07" | "10");
    // Same length and form: the text orders as the dates do.
    well_formed && version >= MINIMUM_API_VERSION
}

/// Where a crate keeps the schema of another API version than the bundled
/// one, relative to its `Cargo.toml`. `graphql!` uses it when it exists.
pub const PROJECT_SCHEMA_PATH: &str = "graphql/admin.schema.json";

/// The environment variable naming another schema file for `graphql!`,
/// relative to the `Cargo.toml` of the crate being compiled, or absolute.
pub const SCHEMA_ENV: &str = "SHOPIFY_GRAPHQL_SCHEMA";
