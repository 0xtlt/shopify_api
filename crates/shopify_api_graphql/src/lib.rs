//! Compile-time support for the typed Shopify Admin GraphQL operations of
//! [`shopify_api`](https://docs.rs/shopify_api).
//!
//! [`analyze`](fn@analyze) checks an operation against a [`Schema`] and
//! describes the shape of its variables and of its result; [`codegen`] turns
//! that description into Rust types. [`literal`] checks the values written in
//! Rust for input types (`{ title: "x" }`) and writes the code that sends
//! them. The `graphql!` and `input!` macros are a thin layer over them.
//! [`project`] finds the API version and the schema file of a crate, and
//! downloads that version's schema (with the `download` feature). No schema
//! is bundled.
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

pub mod analyze;
pub mod codegen;
pub mod literal;
pub mod project;
mod restrict;
pub mod schema;

pub use analyze::{
    Deprecation, Error, FieldBase, Operation, OperationKind, Scalar, SelectedField, Selection,
    Variable, VariableBase, analyze,
};
pub use project::{
    PROJECT_SCHEMA_PATH, SCHEMA_ENV, SchemaSource, latest_api_version, schema_source,
};
pub use schema::{Field, INTROSPECTION_QUERY, InputValue, Schema, SchemaError, TypeDef, TypeRef};

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
