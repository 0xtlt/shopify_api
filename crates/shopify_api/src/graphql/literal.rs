//! What the code `graphql!` and `input!` generate for a literal value
//! (`{ title: "x" }`, `[a, b]`, `raw(…)`) refers to. Not a public API.
//!
//! An object literal is a list of fields nested in tuples,
//! `Object((Field("id", id), (Optional("vendor", vendor), ())))`, written in
//! its order; the macros check its fields and their values first.

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::value::RawValue;

use crate::Maybe;

/// The JSON of a value the macros accept.
pub fn to_raw(value: &impl Serialize) -> Box<RawValue> {
    // The macros only accept strings, numbers, booleans, `serde_json::Value`,
    // input objects, and lists and objects of them: none fails to serialize.
    serde_json::value::to_raw_value(value)
        .expect("the values `graphql!` and `input!` accept always serialize")
}

#[derive(Debug)]
pub struct Object<F>(pub F);

#[derive(Debug)]
pub struct Field<V>(pub &'static str, pub V);

/// `name?: value`: not sent when absent.
#[derive(Debug)]
pub struct Optional<V>(pub &'static str, pub Maybe<V>);

pub trait Fields {
    fn write<M: SerializeMap>(&self, map: &mut M) -> Result<(), M::Error>;
}

impl Fields for () {
    fn write<M: SerializeMap>(&self, _: &mut M) -> Result<(), M::Error> {
        Ok(())
    }
}

impl<V: Serialize, R: Fields> Fields for (Field<V>, R) {
    fn write<M: SerializeMap>(&self, map: &mut M) -> Result<(), M::Error> {
        map.serialize_entry(self.0.0, &self.0.1)?;
        self.1.write(map)
    }
}

impl<V: Serialize, R: Fields> Fields for (Optional<V>, R) {
    fn write<M: SerializeMap>(&self, map: &mut M) -> Result<(), M::Error> {
        match &self.0.1 {
            Maybe::Absent => {}
            Maybe::Null => map.serialize_entry(self.0.0, &())?,
            Maybe::Value(value) => map.serialize_entry(self.0.0, value)?,
        }
        self.1.write(map)
    }
}

impl<F: Fields> Serialize for Object<F> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        self.0.write(&mut map)?;
        map.end()
    }
}

/// `[a, b]`: its items, of types that may differ, nested in tuples.
#[derive(Debug)]
pub struct List<I>(pub I);

pub trait Items {
    fn write<S: SerializeSeq>(&self, seq: &mut S) -> Result<(), S::Error>;
}

impl Items for () {
    fn write<S: SerializeSeq>(&self, _: &mut S) -> Result<(), S::Error> {
        Ok(())
    }
}

impl<V: Serialize, R: Items> Items for (V, R) {
    fn write<S: SerializeSeq>(&self, seq: &mut S) -> Result<(), S::Error> {
        seq.serialize_element(&self.0)?;
        self.1.write(seq)
    }
}

impl<I: Items> Serialize for List<I> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut seq = serializer.serialize_seq(None)?;
        self.0.write(&mut seq)?;
        seq.end()
    }
}

/// The value of `raw(…)`: any string, sent as the enum value.
#[diagnostic::on_unimplemented(
    message = "`raw(…)` takes the enum value as a string, not `{Self}`",
    label = "a `String`, a `&str` or anything else `Into<String>`"
)]
pub trait RawString {
    fn into_string(self) -> String;
}

impl<T: Into<String>> RawString for T {
    fn into_string(self) -> String {
        self.into()
    }
}

pub fn raw(value: impl RawString) -> String {
    value.into_string()
}

/// `field?: raw(…)` on a field that may be `null`.
#[diagnostic::on_unimplemented(
    message = "`field?: raw(…)` takes an `Option` or a `shopify_api::Maybe` of a string, not `{Self}`",
    label = "`None` and `Maybe::Absent` leave the field out"
)]
pub trait RawOptional {
    fn into_maybe(self) -> Maybe<String>;
}

impl<T: RawString> RawOptional for Option<T> {
    fn into_maybe(self) -> Maybe<String> {
        option(self.map(RawString::into_string))
    }
}

impl<T: RawString> RawOptional for Maybe<T> {
    fn into_maybe(self) -> Maybe<String> {
        match self {
            Maybe::Absent => Maybe::Absent,
            Maybe::Null => Maybe::Null,
            Maybe::Value(value) => Maybe::Value(value.into_string()),
        }
    }
}

pub fn raw_optional(value: impl RawOptional) -> Maybe<String> {
    value.into_maybe()
}

/// `field?: raw(…)` on a non-null field with a default: an `Option` only,
/// since `null` is refused there.
#[diagnostic::on_unimplemented(
    message = "`field?: raw(…)` takes an `Option` of a string here, not `{Self}`",
    label = "the field is non-null: `None` leaves it out, and `null` is refused"
)]
pub trait RawOption {
    fn into_maybe(self) -> Maybe<String>;
}

impl<T: RawString> RawOption for Option<T> {
    fn into_maybe(self) -> Maybe<String> {
        option(self.map(RawString::into_string))
    }
}

pub fn raw_option(value: impl RawOption) -> Maybe<String> {
    value.into_maybe()
}

/// The value of `field?: value`: `None` leaves the field out.
pub trait IntoMaybe<T> {
    fn into_maybe(self) -> Maybe<T>;
}

impl<T> IntoMaybe<T> for Option<T> {
    fn into_maybe(self) -> Maybe<T> {
        option(self)
    }
}

impl<T> IntoMaybe<T> for Maybe<T> {
    fn into_maybe(self) -> Maybe<T> {
        self
    }
}

fn option<V>(value: Option<V>) -> Maybe<V> {
    match value {
        Some(value) => Maybe::Value(value),
        None => Maybe::Absent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn objects_and_lists_serialize_in_their_order_and_leave_out_absent_fields() {
        let object = Object((
            Field("type", "single_line_text_field"),
            (
                Optional("a", Maybe::<&str>::Absent),
                (
                    Optional("b", Maybe::<&str>::Null),
                    (
                        Optional("c", option(Some(1))),
                        (Field("list", List(("x", (2, ((), ()))))), ()),
                    ),
                ),
            ),
        ));
        assert_eq!(
            to_raw(&object).get(),
            r#"{"type":"single_line_text_field","b":null,"c":1,"list":["x",2,null]}"#
        );
        assert_eq!(to_raw(&Object(())).get(), "{}");
        assert_eq!(to_raw(&List(())).get(), "[]");
        assert_eq!(
            raw_optional(Maybe::Value("A")),
            Maybe::Value("A".to_string())
        );
        assert_eq!(raw_option(None::<&str>), Maybe::Absent);
    }
}
