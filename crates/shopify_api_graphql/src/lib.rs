//! Compile-time support for the typed Shopify Admin GraphQL operations of
//! [`shopify_api`](https://docs.rs/shopify_api).
//!
//! **An implementation detail of `shopify_api_macros` and of the
//! `shopify-api` command, with no semver guarantee**: any release may change
//! its items. `shopify_api` depends on it with an exact `=` version. Depend on
//! [`shopify_api`](https://crates.io/crates/shopify_api), whose `graphql!`
//! and `input!` macros use it, not on this crate.
//!
//! `analyze` checks an operation against a `Schema` and describes the shape
//! of its variables and of its result; `codegen` turns that description into
//! Rust types. `literal` checks the values written in Rust for input types
//! (`{ title: "x" }`) and writes the code that sends them. `project` finds
//! the API version and the schema file of a crate, and downloads that
//! version's schema (with the `download` feature). No schema is bundled.
//!
//! `SHOPIFY_API_INTERNAL_TEST_SCHEMA` (`project::TEST_SCHEMA_ENV`) is
//! internal to this repository's tests, not part of the API: a schema file
//! `graphql!` then uses for every package.
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

#[doc(hidden)]
pub mod analyze;
#[doc(hidden)]
pub mod codegen;
#[doc(hidden)]
pub mod literal;
#[doc(hidden)]
pub mod project;
mod restrict;
#[doc(hidden)]
pub mod schema;

#[doc(hidden)]
pub use analyze::{
    Deprecation, Error, FieldBase, Operation, OperationKind, Scalar, SelectedField, Selection,
    Variable, VariableBase, analyze,
};
#[doc(hidden)]
pub use project::{
    PROJECT_SCHEMA_PATH, SchemaSource, TEST_SCHEMA_ENV, latest_api_version, schema_source,
};
#[doc(hidden)]
pub use schema::{Field, INTROSPECTION_QUERY, InputValue, Schema, SchemaError, TypeDef, TypeRef};

/// The oldest Admin API version `shopify_api` calls.
#[doc(hidden)]
pub const MINIMUM_API_VERSION: &str = "2026-04";

/// Whether `version` is an Admin API version `shopify_api` calls: a quarterly
/// release such as `2026-10`, from [`MINIMUM_API_VERSION`] on, or `unstable`.
#[doc(hidden)]
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
