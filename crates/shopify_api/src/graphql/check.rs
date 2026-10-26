//! What a value must be for Shopify to read it as it was written, checked
//! when the value is built: a finite `Float`, an `Int` of 32 bits, no `null`
//! for a non-null `JSON`, a value that serializes. A value that is not is
//! kept as an [`Invalid`], and the operation that holds it fails with
//! [`Error::InvalidValue`] before anything is sent.

use std::borrow::Cow;
use std::marker::PhantomData;

use serde::{Serialize, Serializer};
use serde_json::Value;

use crate::error::{SensitiveDetail, is_graphql_name};
use crate::{Error, InputObject};

/// Why a value is refused, in [`Error::InvalidValue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum InvalidValueReason {
    /// A `Float` that is NaN or infinite, which JSON writes as `null`.
    NotFiniteFloat,
    /// An `Int` outside of the 32 bits of GraphQL's `Int`.
    IntOutOfRange,
    /// A top-level `null` where the type is a non-null `JSON`.
    NullJson,
    /// A value whose `Serialize` implementation failed.
    Unserializable,
    /// An enum value or an input field name that is not a GraphQL name: a
    /// bulk query writes its variables into its text, where it would not
    /// read as written.
    NotAGraphqlName,
    /// An enum value `true`, `false` or `null`, which GraphQL does not
    /// allow: written into a bulk query, it would read as a boolean or as
    /// `null`.
    ReservedEnumValue,
}

impl std::fmt::Display for InvalidValueReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NotFiniteFloat => "not a finite Float",
            Self::IntOutOfRange => "Int out of 32-bit range",
            Self::NullJson => "null for a non-null JSON field",
            Self::Unserializable => "could not serialize",
            Self::NotAGraphqlName => {
                "not a GraphQL name, as an enum value or a field written into a bulk query must be"
            }
            Self::ReservedEnumValue => {
                "`true`, `false` or `null`, which GraphQL does not allow as an enum value"
            }
        })
    }
}

/// A value that is refused, and where it stands. Not a public API.
#[derive(Clone, Debug)]
pub struct Invalid {
    /// From the value out: the innermost step first.
    path: Vec<Step>,
    reason: InvalidValueReason,
    detail: Option<SensitiveDetail>,
}

#[derive(Clone, Debug)]
enum Step {
    Field(&'static str),
    Index(usize),
}

impl Invalid {
    pub(crate) fn new(reason: InvalidValueReason) -> Self {
        Self {
            path: Vec::new(),
            reason,
            detail: None,
        }
    }

    pub(crate) fn unserializable(error: &serde_json::Error) -> Self {
        Self {
            detail: Some(SensitiveDetail::new(error.to_string())),
            ..Self::new(InvalidValueReason::Unserializable)
        }
    }

    pub(crate) fn field(mut self, name: &'static str) -> Self {
        self.path.push(Step::Field(name));
        self
    }

    pub(crate) fn index(mut self, index: usize) -> Self {
        self.path.push(Step::Index(index));
        self
    }

    /// `root` and the path from it: `ProductInput.metafields[2].value`.
    fn path_from(&self, root: &str) -> String {
        let mut path = root.to_string();
        for step in self.path.iter().rev() {
            match step {
                Step::Field(name) => {
                    path.push('.');
                    path.push_str(name);
                }
                Step::Index(index) => path.push_str(&format!("[{index}]")),
            }
        }
        path
    }

    /// The message of the error of an input object that is serialized on
    /// its own: it holds no part of the value.
    pub(crate) fn message(&self, input_object: &str) -> String {
        format!(
            "invalid value at `{}`: {}",
            self.path_from(input_object),
            self.reason
        )
    }

    /// The error of the variable `name`: the name is shown when it is a
    /// GraphQL name.
    pub(crate) fn into_error(self, name: &str) -> Error {
        let root = if is_graphql_name(name) {
            format!("${name}")
        } else {
            "$[variable]".to_string()
        };
        Error::InvalidValue {
            path: self.path_from(&root),
            reason: self.reason,
            detail: self.detail,
        }
    }
}

fn int(fits: bool) -> Result<(), Invalid> {
    if fits {
        Ok(())
    } else {
        Err(Invalid::new(InvalidValueReason::IntOutOfRange))
    }
}

fn float(value: f64) -> Result<(), Invalid> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(Invalid::new(InvalidValueReason::NotFiniteFloat))
    }
}

fn json(value: &Value) -> Result<(), Invalid> {
    if value.is_null() {
        Err(Invalid::new(InvalidValueReason::NullJson))
    } else {
        Ok(())
    }
}

fn items<'a, T: 'a>(
    items: impl IntoIterator<Item = &'a T>,
    check: impl Fn(&T) -> Result<(), Invalid>,
) -> Result<(), Invalid> {
    for (index, item) in items.into_iter().enumerate() {
        check(item).map_err(|invalid| invalid.index(index))?;
    }
    Ok(())
}

/// The Rust type of a GraphQL variable, as [`graphql!`](crate::graphql!)
/// picks it: `String` (strings, IDs, enum values and Shopify's
/// string-encoded scalars), `i64` (`Int`), `f64` (`Float`), `bool`, `Value`
/// (`JSON`), an [`InputObject`], an `Option` of one for a nullable type and
/// a `Vec` for a list.
///
/// Its value is checked when the variable is set: an `i64` must fit in the
/// 32 bits of a GraphQL `Int`, an `f64` must be finite, a `Value` must not
/// be `null` where its type is non-null. A value that is not makes the
/// operation fail with [`Error::InvalidValue`] before it is sent. This
/// trait is sealed.
pub trait VariableType: Serialize + sealed::Sealed {}

pub(crate) mod sealed {
    use super::Invalid;

    pub trait Sealed {
        /// Checks the value; `nullable` when its GraphQL type is.
        fn check(&self, nullable: bool) -> Result<(), Invalid>;
    }
}

macro_rules! variable_type {
    ($($ty:ty => |$value:ident, $nullable:ident| $check:expr),+ $(,)?) => {$(
        impl VariableType for $ty {}
        impl sealed::Sealed for $ty {
            fn check(&self, $nullable: bool) -> Result<(), Invalid> {
                let $value = self;
                $check
            }
        }
    )+};
}

variable_type!(
    String => |_value, _nullable| Ok(()),
    bool => |_value, _nullable| Ok(()),
    i64 => |value, _nullable| int(i32::try_from(*value).is_ok()),
    f64 => |value, _nullable| float(*value),
    Value => |value, nullable| if nullable { Ok(()) } else { json(value) },
);

impl<const TYPE: u64> VariableType for InputObject<TYPE> {}
impl<const TYPE: u64> sealed::Sealed for InputObject<TYPE> {
    fn check(&self, _: bool) -> Result<(), Invalid> {
        self.invalid()
    }
}

impl<T: VariableType> VariableType for Option<T> {}
impl<T: VariableType> sealed::Sealed for Option<T> {
    fn check(&self, _: bool) -> Result<(), Invalid> {
        match self {
            Some(value) => value.check(true),
            None => Ok(()),
        }
    }
}

impl<T: VariableType> VariableType for Vec<T> {}
impl<T: VariableType> sealed::Sealed for Vec<T> {
    fn check(&self, _: bool) -> Result<(), Invalid> {
        items(self, |item| item.check(false))
    }
}

/// The value of a Rust expression in a literal of `graphql!` or `input!`,
/// checked for the kind `K` of the place it stands in, and written. Not a
/// public API.
pub trait Validate<K> {
    fn validate(&self) -> Result<(), Invalid>;
    /// Serializes the value: an array of any length as a list, which serde
    /// does past 32 items only for slices.
    fn write<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error>;
}

/// The Rust types of a value of the scalar `K`, lists and `Option` aside:
/// what the check of a place of that scalar accepts. `Site` is a type of
/// the generated code: with it, that no `Option` is accepted is known, and
/// the check may also have an impl for `Option`. Not a public API.
pub trait Accepts<K, Site>: Validate<K> {}

/// A value written by [`Validate::write`].
pub struct Written<'a, V, K>(pub &'a V, PhantomData<fn() -> K>);

impl<'a, V, K> Written<'a, V, K> {
    pub fn new(value: &'a V) -> Self {
        Self(value, PhantomData)
    }
}

impl<V: Validate<K>, K> Serialize for Written<'_, V, K> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.write(serializer)
    }
}

/// The kinds of places: the scalars, `JSON` split by whether `null` is
/// refused at its top, and input objects.
#[derive(Debug)]
pub enum Int {}
#[derive(Debug)]
pub enum Float {}
#[derive(Debug)]
pub enum Text {}
#[derive(Debug)]
pub enum Boolean {}
/// A non-null `JSON`.
#[derive(Debug)]
pub enum Json {}
/// A nullable `JSON`.
#[derive(Debug)]
pub enum MaybeJson {}
#[derive(Debug)]
pub enum Input {}

macro_rules! scalar {
    ($kind:ty: $($ty:ty => |$value:ident| $check:expr),+ $(,)?) => {$(
        impl Validate<$kind> for $ty {
            fn validate(&self) -> Result<(), Invalid> {
                let $value = self;
                $check
            }

            fn write<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                self.serialize(serializer)
            }
        }

        impl<Site> Accepts<$kind, Site> for $ty {}
    )+};
}

scalar!(Int:
    i8 => |_value| Ok(()),
    i16 => |_value| Ok(()),
    i32 => |_value| Ok(()),
    i64 => |value| int(i32::try_from(*value).is_ok()),
    isize => |value| int(i32::try_from(*value).is_ok()),
    u8 => |_value| Ok(()),
    u16 => |_value| Ok(()),
    u32 => |value| int(i32::try_from(*value).is_ok()),
    u64 => |value| int(i32::try_from(*value).is_ok()),
    usize => |value| int(i32::try_from(*value).is_ok()),
);
scalar!(Float:
    f32 => |value| float(f64::from(*value)),
    f64 => |value| float(*value),
    i32 => |_value| Ok(()),
    u32 => |_value| Ok(()),
);
scalar!(Text:
    String => |_value| Ok(()),
    &str => |_value| Ok(()),
    &String => |_value| Ok(()),
    Cow<'_, str> => |_value| Ok(()),
);
scalar!(Boolean: bool => |_value| Ok(()));
scalar!(Json:
    Value => |value| json(value),
    &Value => |value| json(value),
);
scalar!(MaybeJson:
    Value => |_value| Ok(()),
    &Value => |_value| Ok(()),
);

impl<const TYPE: u64> Validate<Input> for InputObject<TYPE> {
    fn validate(&self) -> Result<(), Invalid> {
        self.invalid()
    }

    fn write<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.serialize(serializer)
    }
}

impl<const TYPE: u64> Validate<Input> for &InputObject<TYPE> {
    fn validate(&self) -> Result<(), Invalid> {
        self.invalid()
    }

    fn write<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.serialize(serializer)
    }
}

impl<K, V: Validate<K>> Validate<K> for Option<V> {
    fn validate(&self) -> Result<(), Invalid> {
        match self {
            Some(value) => value.validate(),
            None => Ok(()),
        }
    }

    fn write<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Some(value) => value.write(serializer),
            None => serializer.serialize_none(),
        }
    }
}

impl<K, V: Validate<K>, const N: usize> Validate<K> for [V; N] {
    fn validate(&self) -> Result<(), Invalid> {
        self.as_slice().validate()
    }

    fn write<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_slice().write(serializer)
    }
}

impl<K, V: Validate<K>> Validate<K> for Vec<V> {
    fn validate(&self) -> Result<(), Invalid> {
        self.as_slice().validate()
    }

    fn write<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_slice().write(serializer)
    }
}

impl<K, V: Validate<K>> Validate<K> for &Vec<V> {
    fn validate(&self) -> Result<(), Invalid> {
        self.as_slice().validate()
    }

    fn write<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.as_slice().write(serializer)
    }
}

impl<K, V: Validate<K>> Validate<K> for &[V] {
    fn validate(&self) -> Result<(), Invalid> {
        items(self.iter(), V::validate)
    }

    fn write<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_seq(self.iter().map(Written::<V, K>::new))
    }
}
