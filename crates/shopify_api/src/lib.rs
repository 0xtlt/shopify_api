//! A Shopify Admin GraphQL client for Rust.
//!
//! - **Checked operations.** [`graphql!`] checks a query or a mutation
//!   against Shopify's Admin schema while your crate compiles, and gives its
//!   variables and its result Rust types. A field that does not exist is a
//!   compile error; a deprecated one is a compiler warning with Shopify's reason.
//! - **Tokens handled.** A static token, the client credentials grant, or an
//!   expiring offline token: the client obtains and renews it, once for all
//!   its clones, and saves it where you say ([`TokenStore`]).
//! - **Throttling handled.** A throttled request waits for the time its cost
//!   needs, then goes again ([`RetryPolicy`]).
//! - **The rest of the Admin API work:** cursor [pagination](Shopify::paginate),
//!   [bulk operations](crate::bulk), [webhooks] and
//!   [OAuth](oauth), a [`MockTransport`](testing::MockTransport)
//!   for tests, and the `shopify-api` command, which refreshes the schema
//!   and checks `.graphql` files.
//!
//! ```no_run
//! use shopify_api::{Auth, Shopify, graphql};
//!
//! # async fn run() -> shopify_api::Result<()> {
//! let shopify = Shopify::new("my-shop", Auth::access_token("shpat_…"))?;
//!
//! let data = shopify
//!     .execute(graphql!(
//!         "query Products($first: Int!) {
//!            products(first: $first) { nodes { id title totalInventory } }
//!          }",
//!         first = 10,
//!     ))
//!     .await?;
//!
//! for product in data.products.nodes {
//!     println!("{} has {:?} in stock", product.title, product.total_inventory);
//! }
//! # Ok(())
//! # }
//! ```
//!
//! See the [`graphql!`] macro for the mapping of types, named operations,
//! input values and the schema of other API versions, and [`input!`] for an
//! input object built ahead of a call.
//!
//! # Features
//!
//! | Feature | |
//! | --- | --- |
//! | `rustls` (default) | [`ReqwestTransport`](transport::ReqwestTransport) over rustls. |
//! | `native-tls` | The same over the platform's TLS. |
//! | `webhooks` | [`webhooks`]: verification, parsing, subscriptions. |
//! | `oauth` | [`oauth`]: install links, callbacks, token exchange, session tokens, app proxies. |
//! | `testing` | [`testing::MockTransport`], and the signing helpers of tests. |
//! | `cli` | The `shopify-api` command. |
//! | `full` | Every feature but `native-tls`: `webhooks`, `oauth`, `testing`, `cli`, over rustls. |
//!
//! The client runs on Tokio: the default transport, and the token renewals,
//! which run in tasks of their own, need a Tokio runtime.
#![forbid(unsafe_code)]
#![cfg_attr(docsrs, feature(doc_cfg))]

// The operations of this crate use `graphql!`, whose code refers to `::shopify_api`.
extern crate self as shopify_api;

mod auth;
pub mod bulk;
mod client;
mod decode;
mod error;
mod graphql;
mod pagination;
pub mod schema;
mod shop;
pub mod transport;
mod version;

#[cfg(feature = "oauth")]
#[cfg_attr(docsrs, doc(cfg(feature = "oauth")))]
pub mod oauth;
#[cfg(feature = "testing")]
#[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
pub mod testing;
#[cfg(feature = "webhooks")]
#[cfg_attr(docsrs, doc(cfg(feature = "webhooks")))]
pub mod webhooks;

pub use auth::{AssociatedUser, Auth, TokenData, TokenStore};
pub use client::{RetryPolicy, Shopify, ShopifyBuilder, USER_AGENT};
pub use error::{Error, GraphqlErrors, OAuthErrorCode, RawBody, Result, SensitiveDetail};
pub use graphql::{
    Cost, GraphqlError, InputObject, IntoVariable, InvalidValueReason, Location, Maybe, Operation,
    OperationKind, RawResponse, Response, ThrottleStatus, UserError, VariableType,
};
pub use pagination::Page;
pub use shop::ShopDomain;
pub use version::ApiVersion;

/// Checks a Shopify Admin GraphQL operation against Shopify's schema while
/// the crate compiles, and gives its variables and its result Rust types.
#[doc = include_str!("../docs/graphql.md")]
pub use shopify_api_macros::graphql;

/// Builds an input object of the schema, checked like a literal of
/// [`graphql!`]: `input!(MetafieldsSetInput { key: "color", … })`.
#[doc = include_str!("../docs/input.md")]
pub use shopify_api_macros::input;

/// What the code `graphql!` and `input!` generate refers to. Not a public API.
#[doc(hidden)]
pub mod __private {
    pub use crate::bulk::__private as bulk;
    pub use crate::graphql::literal;
    pub use crate::{InputObject, Maybe, Operation, OperationKind, Result};
    pub use serde;
    pub use serde_json;
}
