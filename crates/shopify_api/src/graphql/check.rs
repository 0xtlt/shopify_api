//! What a value must be for Shopify to read it as it was written, checked
//! when the value is built: a finite `Float`, an `Int` of 32 bits, no `null`
//! for a non-null `JSON`, a value that serializes. A value that is not is
//! kept as an [`Invalid`], and the operation that holds it fails with
//! [`Error::InvalidValue`] before anything is sent.

use std::borrow::Cow;

use serde::Serialize;
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
}

impl std::fmt::Display for InvalidValueReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NotFiniteFloat => "not a finite Float",
            Self::IntOutOfRange => "Int out of 32-bit range",
            Self::NullJson => "null for a non-null JSON field",
            Self::Unserializable => "could not serialize",
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
/// checked for the scalar `K` it stands for: [`Int`], [`Float`], [`Json`]
/// (non-null) or [`Plain`] (anything else). Not a public API.
pub trait Validate<K> {
    fn validate(&self) -> Result<(), Invalid>;
}

#[derive(Debug)]
pub enum Int {}
#[derive(Debug)]
pub enum Float {}
/// A non-null `JSON`.
#[derive(Debug)]
pub enum Json {}
#[derive(Debug)]
pub enum Plain {}

macro_rules! validate {
    ($kind:ty: $($ty:ty => |$value:ident| $check:expr),+ $(,)?) => {$(
        impl Validate<$kind> for $ty {
            fn validate(&self) -> Result<(), Invalid> {
                let $value = self;
                $check
            }
        }
    )+};
}

validate!(Int:
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
validate!(Float:
    f32 => |value| float(f64::from(*value)),
    f64 => |value| float(*value),
    i32 => |_value| Ok(()),
    u32 => |_value| Ok(()),
);
validate!(Json:
    Value => |value| json(value),
    &Value => |value| json(value),
);
validate!(Plain:
    String => |_value| Ok(()),
    &str => |_value| Ok(()),
    &String => |_value| Ok(()),
    Cow<'_, str> => |_value| Ok(()),
    bool => |_value| Ok(()),
    Value => |_value| Ok(()),
    &Value => |_value| Ok(()),
);

impl<const TYPE: u64> Validate<Plain> for InputObject<TYPE> {
    fn validate(&self) -> Result<(), Invalid> {
        self.invalid()
    }
}

impl<const TYPE: u64> Validate<Plain> for &InputObject<TYPE> {
    fn validate(&self) -> Result<(), Invalid> {
        self.invalid()
    }
}

impl<K, V: Validate<K>> Validate<K> for Option<V> {
    fn validate(&self) -> Result<(), Invalid> {
        match self {
            Some(value) => value.validate(),
            None => Ok(()),
        }
    }
}

impl<K, V: Validate<K>> Validate<K> for Vec<V> {
    fn validate(&self) -> Result<(), Invalid> {
        items(self, V::validate)
    }
}

impl<K, V: Validate<K>> Validate<K> for &Vec<V> {
    fn validate(&self) -> Result<(), Invalid> {
        items(*self, V::validate)
    }
}

impl<K, V: Validate<K>> Validate<K> for &[V] {
    fn validate(&self) -> Result<(), Invalid> {
        items(*self, V::validate)
    }
}

impl<K, V: Validate<K>, const N: usize> Validate<K> for [V; N] {
    fn validate(&self) -> Result<(), Invalid> {
        items(self, V::validate)
    }
}
