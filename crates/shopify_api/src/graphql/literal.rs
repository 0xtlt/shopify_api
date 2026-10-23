//! What the code `graphql!` and `input!` generate for a literal value
//! (`{ title: "x" }`, `[a, b]`, `raw(…)`) refers to. Not a public API.
//!
//! An object literal is a list of fields nested in tuples,
//! `Object((Field("id", id), (Optional("vendor", vendor), ())))`, written in
//! its order; the macros check its fields and their values first. A Rust
//! expression in it is a [`Leaf`], checked for the scalar it stands for when
//! the value is built.

use std::marker::PhantomData;

use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::value::RawValue;

pub use super::check::{
    Accepts, Boolean, Float, Input, Int, Invalid, Json, MaybeJson, Text, Validate,
};
use crate::Maybe;

/// The JSON of a value, or why it is refused.
pub fn to_raw(value: &(impl Check + Serialize)) -> Result<Box<RawValue>, Invalid> {
    value.check()?;
    serde_json::value::to_raw_value(value).map_err(|error| Invalid::unserializable(&error))
}

/// A value of a literal, checked before it is serialized.
pub trait Check {
    fn check(&self) -> Result<(), Invalid>;
}

/// `null`, an enum value, the string of `raw(…)`, a literal Rust value
/// whose type cannot be refused.
macro_rules! unchecked {
    ($($ty:ty),+) => {$(
        impl Check for $ty {
            fn check(&self) -> Result<(), Invalid> {
                Ok(())
            }
        }
    )+};
}

unchecked!((), &str, String, bool);

impl<T: Check> Check for &T {
    fn check(&self) -> Result<(), Invalid> {
        (**self).check()
    }
}

/// A Rust expression for a value of the scalar `K`.
#[derive(Debug)]
pub struct Leaf<V, K>(V, PhantomData<fn() -> K>);

pub fn leaf<K, V>(value: V) -> Leaf<V, K> {
    Leaf(value, PhantomData)
}

/// `field?: value`, the value checked as a `K`.
pub fn optional<K, T>(value: Maybe<T>) -> Maybe<Leaf<T, K>> {
    match value {
        Maybe::Absent => Maybe::Absent,
        Maybe::Null => Maybe::Null,
        Maybe::Value(value) => Maybe::Value(leaf(value)),
    }
}

impl<V: Validate<K>, K> Check for Leaf<V, K> {
    fn check(&self) -> Result<(), Invalid> {
        self.0.validate()
    }
}

impl<V: Validate<K>, K> Serialize for Leaf<V, K> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.write(serializer)
    }
}

/// The items of `[for … => …]`, each written in its turn of the loop, and
/// the first that is refused.
#[derive(Debug, Default)]
pub struct Collected {
    items: Vec<Box<RawValue>>,
    invalid: Option<Invalid>,
}

impl Collected {
    pub fn push(&mut self, value: Result<Box<RawValue>, Invalid>) {
        if self.invalid.is_some() {
            return;
        }
        match value {
            Ok(item) => self.items.push(item),
            Err(invalid) => self.invalid = Some(invalid.index(self.items.len())),
        }
    }
}

impl Check for Collected {
    fn check(&self) -> Result<(), Invalid> {
        match &self.invalid {
            Some(invalid) => Err(invalid.clone()),
            None => Ok(()),
        }
    }
}

impl Serialize for Collected {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(&self.items)
    }
}

/// Functions named like the parameters of the method the macros generate,
/// imported where it is declared: a parameter may shadow a function, so no
/// constant of the caller's of the same name turns it into a pattern.
pub mod names {
    macro_rules! names {
        ($($name:ident)*) => {$(
            pub fn $name() {}
        )*};
    }

    names!(
        value0 value1 value2 value3 value4 value5 value6 value7 value8 value9 value10 value11 value12 value13 value14 value15 value16 value17 value18 value19 value20 value21 value22 value23 value24 value25 value26 value27 value28 value29 value30 value31 value32 value33 value34 value35 value36 value37 value38 value39 value40 value41 value42 value43 value44 value45 value46 value47 value48 value49 value50 value51 value52 value53 value54 value55 value56 value57 value58 value59 value60 value61 value62 value63
    );
}

#[derive(Debug)]
pub struct Object<F>(pub F);

#[derive(Debug)]
pub struct Field<V>(pub &'static str, pub V);

/// `name?: value`: not sent when absent.
#[derive(Debug)]
pub struct Optional<V>(pub &'static str, pub Maybe<V>);

pub trait Fields {
    fn check(&self) -> Result<(), Invalid>;
    fn write<M: SerializeMap>(&self, map: &mut M) -> Result<(), M::Error>;
}

impl Fields for () {
    fn check(&self) -> Result<(), Invalid> {
        Ok(())
    }

    fn write<M: SerializeMap>(&self, _: &mut M) -> Result<(), M::Error> {
        Ok(())
    }
}

impl<V: Check + Serialize, R: Fields> Fields for (Field<V>, R) {
    fn check(&self) -> Result<(), Invalid> {
        self.0
            .1
            .check()
            .map_err(|invalid| invalid.field(self.0.0))?;
        self.1.check()
    }

    fn write<M: SerializeMap>(&self, map: &mut M) -> Result<(), M::Error> {
        map.serialize_entry(self.0.0, &self.0.1)?;
        self.1.write(map)
    }
}

impl<V: Check + Serialize, R: Fields> Fields for (Optional<V>, R) {
    fn check(&self) -> Result<(), Invalid> {
        if let Maybe::Value(value) = &self.0.1 {
            value.check().map_err(|invalid| invalid.field(self.0.0))?;
        }
        self.1.check()
    }

    fn write<M: SerializeMap>(&self, map: &mut M) -> Result<(), M::Error> {
        match &self.0.1 {
            Maybe::Absent => {}
            Maybe::Null => map.serialize_entry(self.0.0, &())?,
            Maybe::Value(value) => map.serialize_entry(self.0.0, value)?,
        }
        self.1.write(map)
    }
}

impl<F: Fields> Check for Object<F> {
    fn check(&self) -> Result<(), Invalid> {
        self.0.check()
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
    /// Checks the items, the first being at `index`.
    fn check(&self, index: usize) -> Result<(), Invalid>;
    fn write<S: SerializeSeq>(&self, seq: &mut S) -> Result<(), S::Error>;
}

impl Items for () {
    fn check(&self, _: usize) -> Result<(), Invalid> {
        Ok(())
    }

    fn write<S: SerializeSeq>(&self, _: &mut S) -> Result<(), S::Error> {
        Ok(())
    }
}

impl<V: Check + Serialize, R: Items> Items for (V, R) {
    fn check(&self, index: usize) -> Result<(), Invalid> {
        self.0.check().map_err(|invalid| invalid.index(index))?;
        self.1.check(index + 1)
    }

    fn write<S: SerializeSeq>(&self, seq: &mut S) -> Result<(), S::Error> {
        seq.serialize_element(&self.0)?;
        self.1.write(seq)
    }
}

impl<I: Items> Check for List<I> {
    fn check(&self) -> Result<(), Invalid> {
        self.0.check(0)
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

/// The value of `field?: value`, checked as a `K`: `None` leaves the field
/// out.
pub trait OptionalValue<K> {
    type Value: Validate<K>;
    fn into_maybe(self) -> Maybe<Self::Value>;
}

impl<K, V: Validate<K>> OptionalValue<K> for Option<V> {
    type Value = V;
    fn into_maybe(self) -> Maybe<V> {
        option(self)
    }
}

impl<K, V: Validate<K>> OptionalValue<K> for Maybe<V> {
    type Value = V;
    fn into_maybe(self) -> Maybe<V> {
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
                        Optional("c", optional::<Int, _>(option(Some(1)))),
                        (
                            Field("list", List(("x", (leaf::<Int, _>(2), ((), ()))))),
                            (),
                        ),
                    ),
                ),
            ),
        ));
        assert_eq!(
            to_raw(&object).unwrap().get(),
            r#"{"type":"single_line_text_field","b":null,"c":1,"list":["x",2,null]}"#
        );
        assert_eq!(to_raw(&Object(())).unwrap().get(), "{}");
        assert_eq!(to_raw(&List(())).unwrap().get(), "[]");
        assert_eq!(
            raw_optional(Maybe::Value("A")),
            Maybe::Value("A".to_string())
        );
        assert_eq!(raw_option(None::<&str>), Maybe::Absent);
    }

    #[test]
    fn a_value_is_checked_for_its_scalar_and_refused_with_where_it_stands() {
        use serde_json::Value;

        let refused = |value: Result<Box<RawValue>, Invalid>| value.unwrap_err().message("T");
        assert_eq!(
            refused(to_raw(&Object((
                Optional("j", optional::<Json, _>(Maybe::Value(Value::Null))),
                ()
            )))),
            "invalid value at `T.j`: null for a non-null JSON field"
        );
        assert_eq!(
            refused(to_raw(&Object((
                Field(
                    "list",
                    List((
                        leaf::<Float, _>(1.0),
                        (leaf::<Float, _>(vec![Some(f32::NAN)]), ())
                    ))
                ),
                ()
            )))),
            "invalid value at `T.list[1][0]`: not a finite Float"
        );
        let mut collected = Collected::default();
        for quantity in [1_usize, 2, 1 << 40, 3, usize::MAX] {
            collected.push(to_raw(&Object((
                Field("quantity", leaf::<Int, _>(quantity)),
                (),
            ))));
        }
        assert_eq!(
            refused(to_raw(&Object((Field("items", &collected), ())))),
            "invalid value at `T.items[2].quantity`: Int out of 32-bit range"
        );
        // `null` of a nullable `JSON`, and nulls inside a `JSON`, are sent.
        assert_eq!(
            to_raw(&List((
                leaf::<MaybeJson, _>(Value::Null),
                (leaf::<Json, _>(serde_json::json!([null])), ())
            )))
            .unwrap()
            .get(),
            "[null,[null]]"
        );
    }
}
