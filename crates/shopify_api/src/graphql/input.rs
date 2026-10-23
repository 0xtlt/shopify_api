//! Input objects built by [`input!`](crate::input!), and the field values
//! that may be left out.

use serde::{Serialize, Serializer};
use serde_json::value::RawValue;

/// An input object of the schema, built and checked by
/// [`input!`](crate::input!): `input!(MetafieldsSetInput { … })`.
///
/// `TYPE` identifies the GraphQL input type: a hash of its name, so that
/// every `input!` of `MetafieldsSetInput`, in any module or crate, gives the
/// same Rust type, and values of different input types do not mix. A
/// `graphql!` module names it after the GraphQL type
/// (`metafields_set::MetafieldsSetInput`); messages of `graphql!` and
/// `input!` name the GraphQL type too.
///
/// The value is opaque: it holds the JSON it is sent as, written when it is
/// built.
#[derive(Clone)]
pub struct InputObject<const TYPE: u64> {
    name: &'static str,
    json: Box<RawValue>,
}

impl<const TYPE: u64> InputObject<TYPE> {
    /// What `input!` builds.
    #[doc(hidden)]
    pub fn __new(name: &'static str, fields: &impl Serialize) -> Self {
        Self {
            name,
            json: crate::__private::literal::to_raw(fields),
        }
    }
}

impl<const TYPE: u64> Serialize for InputObject<TYPE> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.json.serialize(serializer)
    }
}

impl<const TYPE: u64> std::fmt::Debug for InputObject<TYPE> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} {}", self.name, self.json.get())
    }
}

/// A field given with `field?: value` in [`graphql!`](crate::graphql!) and
/// [`input!`](crate::input!), when sending `null` is a choice made at run
/// time: an `Option` covers absent or value, `Maybe` adds `null`.
///
/// ```no_run
/// use shopify_api::{Maybe, input};
///
/// # fn vendor(clear: bool, vendor: Option<String>) {
/// let vendor = match (clear, vendor) {
///     (true, _) => Maybe::Null,
///     (false, Some(vendor)) => Maybe::Value(vendor),
///     (false, None) => Maybe::Absent,
/// };
/// let product = input!(ProductUpdateInput { id: "gid://shopify/Product/1", vendor?: vendor });
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Maybe<T> {
    /// The field is not sent: Shopify keeps what it has.
    #[default]
    Absent,
    /// The field is sent as `null`: Shopify clears it.
    Null,
    Value(T),
}

impl<T> From<T> for Maybe<T> {
    fn from(value: T) -> Self {
        Self::Value(value)
    }
}
