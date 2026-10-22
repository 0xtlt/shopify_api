//! Reads JSON as a type, and builds an error that holds nothing of the input.
//!
//! An error of serde or serde_json may quote what it read: a value
//! (`invalid type: string "…"`), a map key in its path, the text a type's own
//! `Deserialize` writes. What [`Failure`] keeps to show is built from this
//! module's own words and from the `&'static str` names a type gives serde
//! (its fields, its variants, its name): a path segment or a quoted name is
//! shown only when it is one of those, never because of what it looks like.
//! serde's whole message is kept apart, in [`Failure::detail`].

use std::cell::RefCell;
use std::fmt;

use serde::de::{
    self, Deserialize, DeserializeSeed, Deserializer, EnumAccess, MapAccess, SeqAccess,
    VariantAccess, Visitor,
};
use serde_json::error::Category;

/// A value that does not read as its type: what is safe to show, and serde's
/// own message, which is not.
pub(crate) struct Failure {
    /// Field names of the type, indexes, and `[key]` for anything else.
    pub(crate) path: String,
    /// What went wrong, in this module's words.
    pub(crate) message: String,
    /// serde's message as it was: it may quote the input.
    pub(crate) detail: String,
}

impl Failure {
    /// The [`Error::Decode`](crate::Error::Decode) of this failure.
    pub(crate) fn into_error(self) -> crate::Error {
        crate::Error::decode(self.path, self.message, Some(self.detail))
    }
}

/// Reads `value` as `T`.
pub(crate) fn from_value<T: de::DeserializeOwned>(value: serde_json::Value) -> crate::Result<T> {
    read(value, None).map_err(Failure::into_error)
}

/// Reads JSON text as `T`.
pub(crate) fn from_slice<T: de::DeserializeOwned>(bytes: &[u8]) -> Result<T, Failure> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = read(&mut deserializer, None)?;
    deserializer
        .end()
        .map_err(|error| failure(&error, &Track::default(), None))?;
    Ok(value)
}

/// Reads line `line` (from 1) of a JSONL file as `T`: the position the
/// error gives is in the file.
pub(crate) fn from_jsonl_line<T: de::DeserializeOwned>(
    bytes: &[u8],
    line: usize,
) -> crate::Result<T> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = read(&mut deserializer, Some(line)).map_err(Failure::into_error)?;
    deserializer
        .end()
        .map_err(|error| failure(&error, &Track::default(), Some(line)).into_error())?;
    Ok(value)
}

/// Writes `value` as JSON and reads it back as `T`: a generated type read as
/// one of this crate's.
pub(crate) fn reserialize<T: de::DeserializeOwned>(
    value: &impl serde::Serialize,
) -> crate::Result<T> {
    let value = serde_json::to_value(value).map_err(|error| {
        crate::Error::decode(
            ".",
            "the answer does not serialize".to_string(),
            Some(error.to_string()),
        )
    })?;
    from_value(value)
}

fn read<'de, D, T>(deserializer: D, line: Option<usize>) -> Result<T, Failure>
where
    D: Deserializer<'de, Error = serde_json::Error>,
    T: Deserialize<'de>,
{
    let track = Track::default();
    T::deserialize(Wrap {
        inner: deserializer,
        chain: &Chain::Root,
        track: &track,
    })
    .map_err(|error| failure(&error, &track, line))
}

fn failure(error: &serde_json::Error, track: &Track, line: Option<usize>) -> Failure {
    let path = track.path.borrow().clone().unwrap_or_default();
    let mut message = describe(error, track);
    if error.line() > 0 {
        let line = line.unwrap_or(error.line());
        message.push_str(&format!(" at line {line} column {}", error.column()));
    } else if let Some(line) = line {
        message.push_str(&format!(" at line {line}"));
    }
    Failure {
        path: render(&path),
        message,
        detail: error.to_string(),
    }
}

/// What went wrong, without anything serde quoted from the input.
fn describe(error: &serde_json::Error, track: &Track) -> String {
    let full = error.to_string();
    // serde_json adds the position to its message; it is shown on its own.
    let position = format!(" at line {} column {}", error.line(), error.column());
    let message = full.strip_suffix(&position).unwrap_or(&full);
    if let Some(known) = FIXED.iter().find(|known| **known == message) {
        return (*known).to_string();
    }
    match error.classify() {
        Category::Io => "the input could not be read".to_string(),
        Category::Eof => "the input ends too early".to_string(),
        Category::Syntax => "invalid JSON".to_string(),
        Category::Data => data_message(message, track),
    }
}

/// serde_json's own messages: none of them quotes the input.
const FIXED: &[&str] = &[
    "EOF while parsing a list",
    "EOF while parsing an object",
    "EOF while parsing a string",
    "EOF while parsing a value",
    "expected `:`",
    "expected `,` or `]`",
    "expected `,` or `}`",
    "expected ident",
    "expected value",
    "expected `\"`",
    "invalid escape",
    "invalid number",
    "number out of range",
    "invalid unicode code point",
    "control character (\\u0000-\\u001F) found while parsing a string",
    "key must be a string",
    "invalid value: expected key to be a number in quotes",
    "float key must be finite (got NaN or +/-inf)",
    "lone leading surrogate in hex escape",
    "trailing comma",
    "trailing characters",
    "unexpected end of hex escape",
    "recursion limit exceeded",
];

/// The kinds serde names an unexpected value by (`Unexpected`'s `Display`).
/// The longer first: `unit variant` before `unit value` is not an issue, but
/// `floating point` must not read as another kind.
const KINDS: &[&str] = &[
    "boolean",
    "integer",
    "floating point",
    "character",
    "string",
    "byte array",
    "unit value",
    "Option value",
    "newtype struct",
    "sequence",
    "map",
    "enum",
    "unit variant",
    "newtype variant",
    "tuple variant",
    "struct variant",
    "null",
];

/// What serde's primitive types and `Value` expect.
const EXPECTED: &[&str] = &[
    "a boolean",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "f32",
    "f64",
    "a character",
    "a string",
    "a borrowed string",
    "a byte array",
    "a borrowed byte array",
    "option",
    "unit",
    "a sequence",
    "a map",
    "any valid JSON value",
    "a JSON number",
];

fn data_message(message: &str, track: &Track) -> String {
    for (prefix, what) in [
        ("invalid type: ", "invalid type"),
        ("invalid value: ", "invalid value"),
    ] {
        if let Some(rest) = message.strip_prefix(prefix) {
            let mut shown = what.to_string();
            if let Some(kind) = KINDS.iter().find(|kind| {
                rest.strip_prefix(**kind)
                    .is_some_and(|after| after.is_empty() || after.starts_with([' ', ',']))
            }) {
                shown.push_str(": ");
                shown.push_str(kind);
            }
            // What the type expects, when it is serde's or the type's name.
            if let Some(expected) = rest.rfind(", expected ").map(|at| &rest[at + 11..])
                && let Some(expected) = expected_by_type(expected, track)
            {
                shown.push_str(", expected ");
                shown.push_str(&expected);
            }
            return shown;
        }
    }
    for (prefix, what) in [
        ("missing field `", "missing field"),
        ("duplicate field `", "duplicate field"),
    ] {
        if let Some(rest) = message.strip_prefix(prefix) {
            // The name a derived type gives serde, never one the input wrote.
            return match rest
                .strip_suffix('`')
                .and_then(|name| track.static_name(name))
            {
                Some(name) => format!("{what} `{name}`"),
                None => what.to_string(),
            };
        }
    }
    for (prefix, what) in [
        ("invalid length ", "invalid length"),
        ("unknown variant `", "unknown variant"),
        ("unknown field `", "unknown field"),
        (
            "data did not match any variant of untagged enum ",
            "no variant of an untagged enum matches",
        ),
    ] {
        if message.starts_with(prefix) {
            return what.to_string();
        }
    }
    "the type's `Deserialize` refused the value".to_string()
}

/// `expected` as this crate writes it, from its tables and the names the
/// type gave serde.
fn expected_by_type(expected: &str, track: &Track) -> Option<String> {
    if let Some(known) = EXPECTED.iter().find(|known| **known == expected) {
        return Some((*known).to_string());
    }
    for kind in ["struct ", "tuple struct ", "unit struct ", "enum "] {
        if let Some(name) = expected
            .strip_prefix(kind)
            .and_then(|name| track.static_name(name))
        {
            return Some(format!("{kind}{name}"));
        }
    }
    None
}

/// One step of the path to a value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Segment {
    /// A field or a variant, by the name its type gives serde.
    Name(&'static str),
    /// An entry of a sequence.
    Index(usize),
    /// A map key, or a name the type does not know: never shown.
    Key,
}

fn render(path: &[Segment]) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let mut shown = String::new();
    for segment in path {
        match segment {
            Segment::Name(name) => {
                if !shown.is_empty() {
                    shown.push('.');
                }
                shown.push_str(name);
            }
            Segment::Index(index) => shown.push_str(&format!("[{index}]")),
            Segment::Key => shown.push_str("[key]"),
        }
    }
    shown
}

/// The path to the value being read, on the stack.
enum Chain<'a> {
    Root,
    Step {
        parent: &'a Chain<'a>,
        segment: Segment,
    },
}

impl Chain<'_> {
    fn segments(&self) -> Vec<Segment> {
        let mut segments = Vec::new();
        let mut chain = self;
        while let Chain::Step { parent, segment } = chain {
            segments.push(*segment);
            chain = parent;
        }
        segments.reverse();
        segments
    }
}

/// The path of the first error, and the names the types gave serde.
#[derive(Default)]
struct Track {
    path: RefCell<Option<Vec<Segment>>>,
    /// The lists of names seen, once each.
    lists: RefCell<Vec<&'static [&'static str]>>,
    /// The names of the types seen.
    types: RefCell<Vec<&'static str>>,
}

impl Track {
    /// Keeps the path of the innermost error: the first one to get here.
    fn trigger<E>(&self, chain: &Chain<'_>, error: E) -> E {
        let mut path = self.path.borrow_mut();
        if path.is_none() {
            *path = Some(chain.segments());
        }
        error
    }

    fn names(&self, names: &'static [&'static str]) {
        let mut lists = self.lists.borrow_mut();
        if !lists.iter().any(|seen| std::ptr::eq(*seen, names)) {
            lists.push(names);
        }
    }

    fn type_name(&self, name: &'static str) {
        let mut types = self.types.borrow_mut();
        if !types.contains(&name) {
            types.push(name);
        }
    }

    /// `text` as a name a type gave serde, when it is one.
    fn static_name(&self, text: &str) -> Option<&'static str> {
        let lists = self.lists.borrow();
        let types = self.types.borrow();
        lists
            .iter()
            .flat_map(|names| names.iter())
            .chain(types.iter())
            .find(|name| **name == text)
            .copied()
    }
}

/// The name in `names` that `text` is, else [`Segment::Key`].
fn segment_of(text: &str, names: Option<&'static [&'static str]>) -> Segment {
    names
        .and_then(|names| names.iter().find(|name| **name == text))
        .map_or(Segment::Key, |name| Segment::Name(name))
}

struct Wrap<'a, 'b, D> {
    inner: D,
    chain: &'a Chain<'a>,
    track: &'b Track,
}

macro_rules! forward {
    ($($method:ident($($arg:ident: $type:ty),*);)*) => {$(
        fn $method<V: Visitor<'de>>(self, $($arg: $type,)* visitor: V) -> Result<V::Value, D::Error> {
            let (chain, track) = (self.chain, self.track);
            self.inner
                .$method($($arg,)* WrapVisitor { inner: visitor, chain, track, names: None })
                .map_err(|error| track.trigger(chain, error))
        }
    )*};
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for Wrap<'_, '_, D> {
    type Error = D::Error;

    forward! {
        deserialize_any();
        deserialize_bool();
        deserialize_i8();
        deserialize_i16();
        deserialize_i32();
        deserialize_i64();
        deserialize_i128();
        deserialize_u8();
        deserialize_u16();
        deserialize_u32();
        deserialize_u64();
        deserialize_u128();
        deserialize_f32();
        deserialize_f64();
        deserialize_char();
        deserialize_str();
        deserialize_string();
        deserialize_bytes();
        deserialize_byte_buf();
        deserialize_option();
        deserialize_unit();
        deserialize_seq();
        deserialize_tuple(len: usize);
        deserialize_map();
        deserialize_identifier();
        deserialize_ignored_any();
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.track.type_name(name);
        let (chain, track) = (self.chain, self.track);
        self.inner
            .deserialize_unit_struct(name, WrapVisitor::new(visitor, chain, track, None))
            .map_err(|error| track.trigger(chain, error))
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.track.type_name(name);
        let (chain, track) = (self.chain, self.track);
        self.inner
            .deserialize_newtype_struct(name, WrapVisitor::new(visitor, chain, track, None))
            .map_err(|error| track.trigger(chain, error))
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        len: usize,
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.track.type_name(name);
        let (chain, track) = (self.chain, self.track);
        self.inner
            .deserialize_tuple_struct(name, len, WrapVisitor::new(visitor, chain, track, None))
            .map_err(|error| track.trigger(chain, error))
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.track.type_name(name);
        self.track.names(fields);
        let (chain, track) = (self.chain, self.track);
        self.inner
            .deserialize_struct(
                name,
                fields,
                WrapVisitor::new(visitor, chain, track, Some(fields)),
            )
            .map_err(|error| track.trigger(chain, error))
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, D::Error> {
        self.track.type_name(name);
        self.track.names(variants);
        let (chain, track) = (self.chain, self.track);
        self.inner
            .deserialize_enum(
                name,
                variants,
                WrapVisitor::new(visitor, chain, track, Some(variants)),
            )
            .map_err(|error| track.trigger(chain, error))
    }

    fn is_human_readable(&self) -> bool {
        self.inner.is_human_readable()
    }
}

struct WrapVisitor<'a, 'b, X> {
    inner: X,
    chain: &'a Chain<'a>,
    track: &'b Track,
    /// The fields of a struct, the variants of an enum.
    names: Option<&'static [&'static str]>,
}

impl<'a, 'b, X> WrapVisitor<'a, 'b, X> {
    fn new(
        inner: X,
        chain: &'a Chain<'a>,
        track: &'b Track,
        names: Option<&'static [&'static str]>,
    ) -> Self {
        Self {
            inner,
            chain,
            track,
            names,
        }
    }
}

macro_rules! forward_visit {
    ($($method:ident($type:ty);)*) => {$(
        fn $method<E: de::Error>(self, value: $type) -> Result<X::Value, E> {
            self.inner.$method(value)
        }
    )*};
}

impl<'de, X: Visitor<'de>> Visitor<'de> for WrapVisitor<'_, '_, X> {
    type Value = X::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.expecting(formatter)
    }

    forward_visit! {
        visit_bool(bool);
        visit_i8(i8);
        visit_i16(i16);
        visit_i32(i32);
        visit_i64(i64);
        visit_i128(i128);
        visit_u8(u8);
        visit_u16(u16);
        visit_u32(u32);
        visit_u64(u64);
        visit_u128(u128);
        visit_f32(f32);
        visit_f64(f64);
        visit_char(char);
        visit_str(&str);
        visit_borrowed_str(&'de str);
        visit_string(String);
        visit_bytes(&[u8]);
        visit_borrowed_bytes(&'de [u8]);
        visit_byte_buf(Vec<u8>);
    }

    fn visit_none<E: de::Error>(self) -> Result<X::Value, E> {
        self.inner.visit_none()
    }

    fn visit_unit<E: de::Error>(self) -> Result<X::Value, E> {
        self.inner.visit_unit()
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<X::Value, D::Error> {
        self.inner.visit_some(Wrap {
            inner: deserializer,
            chain: self.chain,
            track: self.track,
        })
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<X::Value, D::Error> {
        self.inner.visit_newtype_struct(Wrap {
            inner: deserializer,
            chain: self.chain,
            track: self.track,
        })
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<X::Value, A::Error> {
        self.inner.visit_seq(WrapSeq {
            inner: seq,
            chain: self.chain,
            track: self.track,
            index: 0,
        })
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<X::Value, A::Error> {
        self.inner.visit_map(WrapMap {
            inner: map,
            chain: self.chain,
            track: self.track,
            names: self.names,
            key: None,
        })
    }

    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<X::Value, A::Error> {
        self.inner.visit_enum(WrapEnum {
            inner: data,
            chain: self.chain,
            track: self.track,
            names: self.names,
        })
    }
}

struct WrapSeed<'a, 'b, S> {
    inner: S,
    chain: &'a Chain<'a>,
    track: &'b Track,
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for WrapSeed<'_, '_, S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<S::Value, D::Error> {
        self.inner.deserialize(Wrap {
            inner: deserializer,
            chain: self.chain,
            track: self.track,
        })
    }
}

struct WrapSeq<'a, 'b, A> {
    inner: A,
    chain: &'a Chain<'a>,
    track: &'b Track,
    index: usize,
}

impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for WrapSeq<'_, '_, A> {
    type Error = A::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, A::Error> {
        let chain = Chain::Step {
            parent: self.chain,
            segment: Segment::Index(self.index),
        };
        self.index += 1;
        let track = self.track;
        self.inner
            .next_element_seed(WrapSeed {
                inner: seed,
                chain: &chain,
                track,
            })
            .map_err(|error| track.trigger(&chain, error))
    }

    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}

struct WrapMap<'a, 'b, A> {
    inner: A,
    chain: &'a Chain<'a>,
    track: &'b Track,
    names: Option<&'static [&'static str]>,
    /// The segment of the last key read.
    key: Option<Segment>,
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for WrapMap<'_, '_, A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, A::Error> {
        self.key = None;
        let names = self.names;
        let result = self.inner.next_key_seed(KeySeed {
            inner: seed,
            names,
            key: &mut self.key,
        });
        result.map_err(|error| {
            let chain = Chain::Step {
                parent: self.chain,
                segment: self.key.unwrap_or(Segment::Key),
            };
            self.track.trigger(&chain, error)
        })
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, A::Error> {
        let chain = Chain::Step {
            parent: self.chain,
            segment: self.key.take().unwrap_or(Segment::Key),
        };
        let track = self.track;
        self.inner
            .next_value_seed(WrapSeed {
                inner: seed,
                chain: &chain,
                track,
            })
            .map_err(|error| track.trigger(&chain, error))
    }

    fn size_hint(&self) -> Option<usize> {
        self.inner.size_hint()
    }
}

struct WrapEnum<'a, 'b, A> {
    inner: A,
    chain: &'a Chain<'a>,
    track: &'b Track,
    names: Option<&'static [&'static str]>,
}

impl<'a, 'b, 'de, A: EnumAccess<'de>> EnumAccess<'de> for WrapEnum<'a, 'b, A> {
    type Error = A::Error;
    type Variant = WrapVariant<'a, 'b, A::Variant>;

    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), A::Error> {
        let mut key = None;
        let (chain, track) = (self.chain, self.track);
        let (value, variant) = self
            .inner
            .variant_seed(KeySeed {
                inner: seed,
                names: self.names,
                key: &mut key,
            })
            .map_err(|error| track.trigger(chain, error))?;
        Ok((
            value,
            WrapVariant {
                inner: variant,
                parent: chain,
                segment: key.unwrap_or(Segment::Key),
                track,
            },
        ))
    }
}

struct WrapVariant<'a, 'b, A> {
    inner: A,
    parent: &'a Chain<'a>,
    segment: Segment,
    track: &'b Track,
}

impl<'de, A: VariantAccess<'de>> VariantAccess<'de> for WrapVariant<'_, '_, A> {
    type Error = A::Error;

    fn unit_variant(self) -> Result<(), A::Error> {
        let chain = Chain::Step {
            parent: self.parent,
            segment: self.segment,
        };
        let track = self.track;
        self.inner
            .unit_variant()
            .map_err(|error| track.trigger(&chain, error))
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, A::Error> {
        let chain = Chain::Step {
            parent: self.parent,
            segment: self.segment,
        };
        let track = self.track;
        self.inner
            .newtype_variant_seed(WrapSeed {
                inner: seed,
                chain: &chain,
                track,
            })
            .map_err(|error| track.trigger(&chain, error))
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, A::Error> {
        let chain = Chain::Step {
            parent: self.parent,
            segment: self.segment,
        };
        let track = self.track;
        self.inner
            .tuple_variant(len, WrapVisitor::new(visitor, &chain, track, None))
            .map_err(|error| track.trigger(&chain, error))
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, A::Error> {
        self.track.names(fields);
        let chain = Chain::Step {
            parent: self.parent,
            segment: self.segment,
        };
        let track = self.track;
        self.inner
            .struct_variant(
                fields,
                WrapVisitor::new(visitor, &chain, track, Some(fields)),
            )
            .map_err(|error| track.trigger(&chain, error))
    }
}

/// Reads a map key or a variant name, and notes which segment it is.
struct KeySeed<'k, S> {
    inner: S,
    names: Option<&'static [&'static str]>,
    key: &'k mut Option<Segment>,
}

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for KeySeed<'_, S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<S::Value, D::Error> {
        self.inner.deserialize(KeyDeserializer {
            inner: deserializer,
            names: self.names,
            key: self.key,
        })
    }
}

struct KeyDeserializer<'k, D> {
    inner: D,
    names: Option<&'static [&'static str]>,
    key: &'k mut Option<Segment>,
}

macro_rules! forward_key {
    ($($method:ident($($arg:ident: $type:ty),*);)*) => {$(
        fn $method<V: Visitor<'de>>(self, $($arg: $type,)* visitor: V) -> Result<V::Value, D::Error> {
            self.inner.$method($($arg,)* KeyVisitor { inner: visitor, names: self.names, key: self.key })
        }
    )*};
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for KeyDeserializer<'_, D> {
    type Error = D::Error;

    forward_key! {
        deserialize_any();
        deserialize_bool();
        deserialize_i8();
        deserialize_i16();
        deserialize_i32();
        deserialize_i64();
        deserialize_i128();
        deserialize_u8();
        deserialize_u16();
        deserialize_u32();
        deserialize_u64();
        deserialize_u128();
        deserialize_f32();
        deserialize_f64();
        deserialize_char();
        deserialize_str();
        deserialize_string();
        deserialize_bytes();
        deserialize_byte_buf();
        deserialize_option();
        deserialize_unit();
        deserialize_unit_struct(name: &'static str);
        deserialize_newtype_struct(name: &'static str);
        deserialize_seq();
        deserialize_tuple(len: usize);
        deserialize_tuple_struct(name: &'static str, len: usize);
        deserialize_map();
        deserialize_struct(name: &'static str, fields: &'static [&'static str]);
        deserialize_enum(name: &'static str, variants: &'static [&'static str]);
        deserialize_identifier();
        deserialize_ignored_any();
    }

    fn is_human_readable(&self) -> bool {
        self.inner.is_human_readable()
    }
}

struct KeyVisitor<'k, X> {
    inner: X,
    names: Option<&'static [&'static str]>,
    key: &'k mut Option<Segment>,
}

macro_rules! capture_key {
    ($($method:ident($type:ty);)*) => {$(
        fn $method<E: de::Error>(self, value: $type) -> Result<X::Value, E> {
            *self.key = Some(Segment::Key);
            self.inner.$method(value)
        }
    )*};
}

impl<'de, X: Visitor<'de>> Visitor<'de> for KeyVisitor<'_, X> {
    type Value = X::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.expecting(formatter)
    }

    capture_key! {
        visit_bool(bool);
        visit_i8(i8);
        visit_i16(i16);
        visit_i32(i32);
        visit_i64(i64);
        visit_i128(i128);
        visit_u8(u8);
        visit_u16(u16);
        visit_u32(u32);
        visit_u64(u64);
        visit_u128(u128);
        visit_f32(f32);
        visit_f64(f64);
        visit_char(char);
        visit_bytes(&[u8]);
        visit_borrowed_bytes(&'de [u8]);
        visit_byte_buf(Vec<u8>);
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<X::Value, E> {
        *self.key = Some(segment_of(value, self.names));
        self.inner.visit_str(value)
    }

    fn visit_borrowed_str<E: de::Error>(self, value: &'de str) -> Result<X::Value, E> {
        *self.key = Some(segment_of(value, self.names));
        self.inner.visit_borrowed_str(value)
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<X::Value, E> {
        *self.key = Some(segment_of(&value, self.names));
        self.inner.visit_string(value)
    }

    fn visit_none<E: de::Error>(self) -> Result<X::Value, E> {
        self.inner.visit_none()
    }

    fn visit_unit<E: de::Error>(self) -> Result<X::Value, E> {
        self.inner.visit_unit()
    }

    fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<X::Value, D::Error> {
        self.inner.visit_some(deserializer)
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<X::Value, D::Error> {
        self.inner.visit_newtype_struct(deserializer)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<X::Value, A::Error> {
        self.inner.visit_seq(seq)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<X::Value, A::Error> {
        self.inner.visit_map(map)
    }

    fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<X::Value, A::Error> {
        self.inner.visit_enum(data)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    fn error<T: de::DeserializeOwned + fmt::Debug>(value: serde_json::Value) -> Failure {
        match read::<_, T>(value, None) {
            Err(failure) => failure,
            Ok(value) => panic!("read {value:?}"),
        }
    }

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Order {
        name: String,
        lines: Vec<Line>,
        totals: BTreeMap<String, u64>,
    }

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    struct Line {
        quantity: u64,
        kind: Kind,
    }

    #[derive(Debug, Deserialize)]
    #[allow(dead_code)]
    enum Kind {
        Physical,
        Digital { url: String },
    }

    #[test]
    fn paths_name_fields_and_hide_keys() {
        let failure = error::<Order>(json!({
            "name": "#1",
            "lines": [{ "quantity": 1, "kind": "Physical" }, { "quantity": "SECRET", "kind": "Physical" }],
            "totals": {},
        }));
        assert_eq!(failure.path, "lines[1].quantity");
        assert_eq!(failure.message, "invalid type: string, expected u64");
        assert!(failure.detail.contains("SECRET"));

        let failure = error::<Order>(json!({
            "name": "#1",
            "lines": [],
            "totals": { "https://x.test/k?sig=SECRET": "SECRET" },
        }));
        assert_eq!(failure.path, "totals[key]");
        assert_eq!(failure.message, "invalid type: string, expected u64");

        let failure = error::<Order>(json!({
            "name": "#1",
            "lines": [{ "quantity": 1, "kind": { "Digital": { "url": 3 } } }],
            "totals": {},
        }));
        assert_eq!(failure.path, "lines[0].kind.Digital.url");
        assert_eq!(failure.message, "invalid type: integer, expected a string");
    }

    #[test]
    fn messages_keep_only_names_the_types_give() {
        let failure = error::<Order>(json!({ "name": "#1", "lines": [] }));
        assert_eq!(
            (failure.path.as_str(), failure.message.as_str()),
            (".", "missing field `totals`")
        );
        let failure = error::<Kind>(json!("shpat_SECRET"));
        assert_eq!(failure.message, "unknown variant");
        assert!(failure.detail.contains("shpat_SECRET"));

        #[derive(Debug)]
        struct Custom;
        impl<'de> Deserialize<'de> for Custom {
            fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let text = String::deserialize(deserializer)?;
                Err(de::Error::custom(format!(
                    "missing field `{text}` and invalid type: string {text:?}"
                )))
            }
        }
        // Its message starts like serde's: the name it quotes is not a field's.
        let failure = error::<Custom>(json!("shpat_SECRET"));
        assert_eq!(failure.message, "missing field");
        let failure = error::<Custom>(json!("\"\\\"shpat_SECRET\\\""));
        assert!(!failure.message.contains("SECRET"));
    }

    #[test]
    fn text_errors_give_a_position_and_no_text() {
        let failure = from_slice::<Order>(br#"{"name": "shpat_SECRET" "#)
            .err()
            .unwrap();
        assert_eq!(
            failure.message,
            "EOF while parsing an object at line 1 column 24"
        );
        let failure = from_slice::<Order>(br#"{"name": shpat_SECRET}"#)
            .err()
            .unwrap();
        assert_eq!(failure.message, "expected value at line 1 column 10");
        let failure = from_slice::<u64>(b"1 shpat").err().unwrap();
        assert_eq!(failure.message, "trailing characters at line 1 column 3");
    }
}
