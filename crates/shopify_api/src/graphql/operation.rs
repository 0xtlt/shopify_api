//! Operations, as built by [`graphql!`](crate::graphql!) or by hand.

use std::borrow::Cow;
use std::marker::PhantomData;

use serde::ser::{Serialize, SerializeMap, Serializer};
use serde_json::value::RawValue;
use serde_json::{Map, Value};

use super::check::{Invalid, VariableType};
use crate::{ApiVersion, Error, Result};

/// A query or a mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OperationKind {
    Query,
    Mutation,
}

impl OperationKind {
    /// Reads the kind of a document: a mutation as soon as one of its
    /// operations is one, so that a document that holds a mutation is never
    /// sent twice by a retry.
    ///
    /// It fails safe: a document it can't read to the end (a subscription, a
    /// typo, an unclosed string or block…) is a [`Mutation`](Self::Mutation)
    /// too, so it is never retried either. Only a document made of queries,
    /// fragments and `{ … }` shorthands is a [`Query`](Self::Query).
    pub fn of(document: &str) -> Self {
        match read_kind(document) {
            Some(kind) => kind,
            None => Self::Mutation,
        }
    }

    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Mutation)
    }
}

/// The kind of the document, `None` when it can't be read.
fn read_kind(document: &str) -> Option<OperationKind> {
    let mut lexer = Lexer {
        text: document,
        position: 0,
    };
    loop {
        match lexer.next()? {
            None => return Some(OperationKind::Query),
            Some(Token::Punctuator(b'{')) => lexer.skip_selection_set()?,
            Some(Token::Name("query" | "fragment")) => {
                lexer.skip_to_selection_set()?;
                lexer.skip_selection_set()?;
            }
            Some(Token::Name("mutation")) => return Some(OperationKind::Mutation),
            Some(_) => return None,
        }
    }
}

/// The tokens that tell where a definition starts and ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Name(&'a str),
    /// `{`, `}`, `(`, `)`, `[` or `]`.
    Punctuator(u8),
    /// Strings, numbers and every other punctuator.
    Other,
}

/// Just enough of a GraphQL lexer to find the definitions of a document.
struct Lexer<'a> {
    text: &'a str,
    position: usize,
}

impl<'a> Lexer<'a> {
    fn rest(&self) -> &'a [u8] {
        &self.text.as_bytes()[self.position..]
    }

    /// Skips white space, line terminators, commas, comments and byte order marks.
    fn skip_ignored(&mut self) {
        loop {
            let rest = self.rest();
            match rest.first() {
                Some(b' ' | b'\t' | b'\n' | b'\r' | b',') => self.position += 1,
                Some(0xEF) if rest.starts_with("\u{feff}".as_bytes()) => self.position += 3,
                // A comment runs to the next line terminator: `\n`, `\r` or `\r\n`.
                Some(b'#') => {
                    self.position += rest
                        .iter()
                        .position(|byte| matches!(byte, b'\n' | b'\r'))
                        .unwrap_or(rest.len());
                }
                _ => return,
            }
        }
    }

    /// The next token: `None` when the text can't be read, `Some(None)` at its end.
    fn next(&mut self) -> Option<Option<Token<'a>>> {
        self.skip_ignored();
        let rest = self.rest();
        let Some(&first) = rest.first() else {
            return Some(None);
        };
        let start = self.position;
        let token = match first {
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => {
                let length = rest
                    .iter()
                    .position(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
                    .unwrap_or(rest.len());
                self.position += length;
                Token::Name(&self.text[start..self.position])
            }
            b'"' if rest.starts_with(b"\"\"\"") => {
                // A block string: its only escape is `\"""`.
                let mut index = 3;
                loop {
                    let tail = rest.get(index..)?;
                    if tail.is_empty() {
                        return None;
                    } else if tail.starts_with(b"\\\"\"\"") {
                        index += 4;
                    } else if tail.starts_with(b"\"\"\"") {
                        break;
                    } else {
                        index += 1;
                    }
                }
                self.position += index + 3;
                Token::Other
            }
            b'"' => {
                let mut index = 1;
                loop {
                    match rest.get(index)? {
                        b'\\' => index += 2,
                        b'"' => break,
                        b'\n' | b'\r' => return None,
                        _ => index += 1,
                    }
                }
                self.position += index + 1;
                Token::Other
            }
            b'{' | b'}' | b'(' | b')' | b'[' | b']' => {
                self.position += 1;
                Token::Punctuator(first)
            }
            b'0'..=b'9' | b'-' => {
                let length = rest
                    .iter()
                    .position(|byte| {
                        !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-'))
                    })
                    .unwrap_or(rest.len());
                self.position += length;
                Token::Other
            }
            _ => {
                // Any other punctuator, or a character GraphQL doesn't allow here.
                let length = self.text[start..].chars().next().map_or(1, char::len_utf8);
                self.position += length;
                Token::Other
            }
        };
        Some(Some(token))
    }

    /// Skips the header of a definition (name, variables and their default
    /// values, directives), up to and with the `{` of its selection set.
    fn skip_to_selection_set(&mut self) -> Option<()> {
        let mut depth = 0usize;
        loop {
            match self.next()?? {
                Token::Punctuator(b'{') if depth == 0 => return Some(()),
                Token::Punctuator(b'{' | b'(' | b'[') => depth += 1,
                Token::Punctuator(_) => depth = depth.checked_sub(1)?,
                _ => {}
            }
        }
    }

    /// Skips a selection set, its `{` already read.
    fn skip_selection_set(&mut self) -> Option<()> {
        let mut depth = 1usize;
        loop {
            match self.next()?? {
                Token::Punctuator(b'{') => depth += 1,
                Token::Punctuator(b'}') => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(());
                    }
                }
                _ => {}
            }
        }
    }
}

/// An operation, its variables, and the type `D` of its `data`.
///
/// [`graphql!`](crate::graphql!) builds it after checking the operation
/// against Shopify's schema; [`Shopify::execute`](crate::Shopify::execute)
/// runs it. [`Operation::new`] builds one from any text, checked by nothing.
pub struct Operation<D> {
    query: Cow<'static, str>,
    kind: OperationKind,
    name: Option<Cow<'static, str>>,
    /// The Admin API version of the schema the operation was checked against.
    checked_against: Option<&'static str>,
    /// Each variable as the JSON it is sent as, in the order it was set.
    variables: std::result::Result<Vec<(String, Box<RawValue>)>, VariablesFailure>,
    data: PhantomData<fn() -> D>,
}

impl<D> Operation<D> {
    /// An operation that nothing checked: its result is read as `D` when it runs.
    pub fn new(query: impl Into<Cow<'static, str>>) -> Self {
        let query = query.into();
        Self {
            kind: OperationKind::of(&query),
            query,
            name: None,
            checked_against: None,
            variables: Ok(Vec::new()),
            data: PhantomData,
        }
    }

    /// What `graphql!` builds.
    #[doc(hidden)]
    pub fn __checked(
        query: &'static str,
        kind: OperationKind,
        name: Option<&'static str>,
        checked_against: &'static str,
    ) -> Self {
        Self {
            query: Cow::Borrowed(query),
            kind,
            name: name.map(Cow::Borrowed),
            checked_against: Some(checked_against),
            variables: Ok(Vec::new()),
            data: PhantomData,
        }
    }

    /// The text of the operation.
    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn kind(&self) -> OperationKind {
        self.kind
    }

    /// The name of the operation, sent as `operationName`.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Names the operation: Shopify shows the name in its logs.
    pub fn with_name(mut self, name: impl Into<Cow<'static, str>>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// The Admin API version of the schema `graphql!` checked the operation
    /// against; `None` for an operation built with [`Operation::new`].
    pub fn checked_against(&self) -> Option<&'static str> {
        self.checked_against
    }

    /// The Admin API version the operation is sent to when the client pins
    /// none: the one it was checked against, else [`ApiVersion::DEFAULT`].
    pub fn api_version(&self) -> Result<ApiVersion> {
        match self.checked_against {
            Some(version) => ApiVersion::new(version),
            None => Ok(ApiVersion::DEFAULT),
        }
    }

    fn set(
        mut self,
        name: &str,
        value: std::result::Result<Box<RawValue>, VariablesFailure>,
    ) -> Self {
        match (&mut self.variables, value) {
            (Ok(variables), Ok(value)) => match variables.iter_mut().find(|(set, _)| set == name) {
                Some((_, set)) => *set = value,
                None => variables.push((name.to_string(), value)),
            },
            (Ok(_), Err(failure)) => self.variables = Err(failure),
            (Err(_), _) => {}
        }
        self
    }

    fn set_json(self, name: &str, value: &impl Serialize) -> Self {
        let value = serde_json::value::to_raw_value(value)
            .map_err(|error| VariablesFailure::of(Error::variables(Some(name), &error)));
        self.set(name, value)
    }

    fn set_checked(self, name: &str, value: &impl VariableType) -> Self {
        match value.check(false) {
            Ok(()) => self.set_json(name, value),
            Err(invalid) => self.set(name, Err(VariablesFailure::invalid(name, invalid))),
        }
    }

    /// Sets a variable to a value convertible to `T`, the Rust type of its
    /// GraphQL type.
    ///
    /// The value is checked as [`VariableType`] says: one that Shopify would
    /// not read as written makes the operation fail with
    /// [`Error::InvalidValue`] before it is sent.
    pub fn variable<T: VariableType, V: IntoVariable<T>>(self, name: &str, value: V) -> Self {
        self.set_checked(name, &value.into_variable())
    }

    /// Sets a variable to any serializable value, checked by nothing.
    pub fn json_variable<V: Serialize>(self, name: &str, value: V) -> Self {
        self.set_json(name, &value)
    }

    /// What the `Variables` of a `graphql!` module set a variable with.
    #[doc(hidden)]
    pub fn __variable<T: VariableType>(self, name: &str, value: T) -> Self {
        self.set_checked(name, &value)
    }

    /// What `graphql!` sets a variable written as a literal to.
    #[doc(hidden)]
    pub fn __literal_variable(
        self,
        name: &str,
        value: std::result::Result<Box<RawValue>, Invalid>,
    ) -> Self {
        let value = value.map_err(|invalid| VariablesFailure::invalid(name, invalid));
        self.set(name, value)
    }

    /// Replaces the variables with the fields of a serializable struct or map.
    pub fn variables<V: Serialize>(mut self, variables: V) -> Self {
        self.variables = variables_of(variables)
            .and_then(|variables| {
                variables
                    .into_iter()
                    .map(
                        |(name, value)| match serde_json::value::to_raw_value(&value) {
                            Ok(value) => Ok((name, value)),
                            Err(error) => Err(Error::variables(Some(&name), &error)),
                        },
                    )
                    .collect()
            })
            .map_err(VariablesFailure::of);
        self
    }

    /// The JSON text the variable `name` is sent as, `None` when it is not
    /// set; an error when a variable was refused, as
    /// [`Shopify::execute`](crate::Shopify::execute) would fail.
    ///
    /// Nothing is parsed back: to read it as a [`Value`], parse it with the
    /// settings you need, such as `serde_json::from_str::<Value>(json)`,
    /// which refuses a value nested deeper than 128.
    pub fn variable_json(&self, name: &str) -> Result<Option<&str>> {
        let variables = self
            .variables
            .as_ref()
            .map_err(|failure| failure.to_error())?;
        Ok(variables
            .iter()
            .find(|(set, _)| set == name)
            .map(|(_, value)| value.get()))
    }

    /// The body as the exact JSON text
    /// [`Shopify::execute`](crate::Shopify::execute) sends: `query`,
    /// `variables` and `operationName`.
    ///
    /// Nothing is parsed back: to read it as a [`Value`], parse it with the
    /// settings you need, such as `serde_json::from_str::<Value>(&body)`,
    /// which refuses a value nested deeper than 128.
    pub fn body(&self) -> Result<String> {
        let variables = self
            .variables
            .as_ref()
            .map_err(|failure| failure.to_error())?;
        let body = Body {
            query: &self.query,
            variables,
            name: self.name.as_deref(),
        };
        serde_json::to_string(&body).map_err(|error| Error::variables(None, &error))
    }
}

struct Body<'a> {
    query: &'a str,
    variables: &'a [(String, Box<RawValue>)],
    name: Option<&'a str>,
}

impl Serialize for Body<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        struct Variables<'a>(&'a [(String, Box<RawValue>)]);
        impl Serialize for Variables<'_> {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.collect_map(self.0.iter().map(|(name, value)| (name, value)))
            }
        }

        let mut body = serializer.serialize_map(None)?;
        body.serialize_entry("query", self.query)?;
        if !self.variables.is_empty() {
            body.serialize_entry("variables", &Variables(self.variables))?;
        }
        if let Some(name) = self.name {
            body.serialize_entry("operationName", name)?;
        }
        body.end()
    }
}

/// The variables of a [`Shopify::graphql_raw`](crate::Shopify::graphql_raw)
/// call or an [`Operation`]: one path, so that both fail the same way.
pub(crate) fn variables_of(variables: impl Serialize) -> Result<Map<String, Value>> {
    match serde_json::to_value(variables) {
        Ok(Value::Object(variables)) => Ok(variables),
        Ok(Value::Null) => Ok(Map::new()),
        Ok(_) => Err(Error::variables_not_an_object()),
        Err(error) => Err(Error::variables(None, &error)),
    }
}

/// An [`Error::Variables`] or an [`Error::InvalidValue`] kept until the
/// operation is sent. Its `Debug` shows no detail.
#[derive(Clone, Debug)]
enum VariablesFailure {
    Variables {
        message: String,
        detail: Option<crate::SensitiveDetail>,
    },
    Invalid {
        variable: String,
        invalid: Invalid,
    },
}

impl VariablesFailure {
    fn of(error: Error) -> Self {
        match error {
            Error::Variables { message, detail } => Self::Variables { message, detail },
            other => Self::Variables {
                message: other.to_string(),
                detail: None,
            },
        }
    }

    fn invalid(variable: &str, invalid: Invalid) -> Self {
        Self::Invalid {
            variable: variable.to_string(),
            invalid,
        }
    }

    fn to_error(&self) -> Error {
        match self {
            Self::Variables { message, detail } => Error::Variables {
                message: message.clone(),
                detail: detail.clone(),
            },
            Self::Invalid { variable, invalid } => invalid.clone().into_error(variable),
        }
    }
}

impl<D> Clone for Operation<D> {
    fn clone(&self) -> Self {
        Self {
            query: self.query.clone(),
            kind: self.kind,
            name: self.name.clone(),
            checked_against: self.checked_against,
            variables: self.variables.clone(),
            data: PhantomData,
        }
    }
}

impl<D> std::fmt::Debug for Operation<D> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Operation")
            .field("kind", &self.kind)
            .field("name", &self.name)
            .field("query", &self.query)
            .field("variables", &self.variables)
            .field("checked_against", &self.checked_against)
            .finish()
    }
}

/// A Rust value accepted for a GraphQL variable whose Rust type is `T`.
///
/// `graphql!` picks `T` from the type of the variable: `String` for strings,
/// IDs, enum values and Shopify's string-encoded scalars, `i64` for `Int`,
/// `f64` for `Float`, `bool` for `Boolean`, `Value` for `JSON`, wrapped in
/// `Option` when the variable is nullable and in `Vec` for lists. The
/// implementations then accept the usual Rust spellings of those values:
/// `&str` for a string, any integer for an `Int`, a bare value for a nullable
/// variable, a slice for a list. The value converted is then checked, as
/// [`VariableType`] says.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a value for a GraphQL variable of Rust type `{T}`",
    label = "the variable expects a value convertible to `{T}`"
)]
pub trait IntoVariable<T> {
    fn into_variable(self) -> T;
}

macro_rules! into_variable {
    ($target:ty: $($source:ty => |$value:ident| $convert:expr),+ $(,)?) => {$(
        impl IntoVariable<$target> for $source {
            fn into_variable(self) -> $target {
                let $value = self;
                $convert
            }
        }

        impl IntoVariable<Option<$target>> for $source {
            fn into_variable(self) -> Option<$target> {
                Some(IntoVariable::<$target>::into_variable(self))
            }
        }
    )+};
}

into_variable!(String:
    String => |value| value,
    &str => |value| value.to_string(),
    &String => |value| value.clone(),
    Cow<'_, str> => |value| value.into_owned(),
);
into_variable!(i64:
    i8 => |value| value.into(),
    i16 => |value| value.into(),
    i32 => |value| value.into(),
    i64 => |value| value,
    u8 => |value| value.into(),
    u16 => |value| value.into(),
    u32 => |value| value.into(),
    // Saturated: out of the range of `Int` all the same, and refused.
    u64 => |value| i64::try_from(value).unwrap_or(i64::MAX),
    usize => |value| i64::try_from(value).unwrap_or(i64::MAX),
    isize => |value| i64::try_from(value).unwrap_or(if value < 0 { i64::MIN } else { i64::MAX }),
);
into_variable!(f64:
    f32 => |value| value.into(),
    f64 => |value| value,
    i32 => |value| value.into(),
    u32 => |value| value.into(),
);
into_variable!(bool: bool => |value| value);
into_variable!(Value:
    Value => |value| value,
    &Value => |value| value.clone(),
);

impl<T, V: IntoVariable<T>> IntoVariable<Option<T>> for Option<V> {
    fn into_variable(self) -> Option<T> {
        self.map(IntoVariable::into_variable)
    }
}

impl<T, V: IntoVariable<T>> IntoVariable<Vec<T>> for Vec<V> {
    fn into_variable(self) -> Vec<T> {
        self.into_iter().map(IntoVariable::into_variable).collect()
    }
}

impl<T, V: IntoVariable<T> + Clone> IntoVariable<Vec<T>> for &[V] {
    fn into_variable(self) -> Vec<T> {
        self.iter()
            .cloned()
            .map(IntoVariable::into_variable)
            .collect()
    }
}

impl<T, V: IntoVariable<T> + Clone> IntoVariable<Vec<T>> for &Vec<V> {
    fn into_variable(self) -> Vec<T> {
        self.as_slice().into_variable()
    }
}

impl<T, V: IntoVariable<T>, const N: usize> IntoVariable<Vec<T>> for [V; N] {
    fn into_variable(self) -> Vec<T> {
        self.into_iter().map(IntoVariable::into_variable).collect()
    }
}

impl<T, V: IntoVariable<T>> IntoVariable<Option<Vec<T>>> for Vec<V> {
    fn into_variable(self) -> Option<Vec<T>> {
        Some(self.into_variable())
    }
}

impl<T, V: IntoVariable<T> + Clone> IntoVariable<Option<Vec<T>>> for &[V] {
    fn into_variable(self) -> Option<Vec<T>> {
        Some(self.into_variable())
    }
}

impl<T, V: IntoVariable<T> + Clone> IntoVariable<Option<Vec<T>>> for &Vec<V> {
    fn into_variable(self) -> Option<Vec<T>> {
        Some(self.into_variable())
    }
}

impl<T, V: IntoVariable<T>, const N: usize> IntoVariable<Option<Vec<T>>> for [V; N] {
    fn into_variable(self) -> Option<Vec<T>> {
        Some(self.into_variable())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn body<D>(operation: &Operation<D>) -> Value {
        serde_json::from_str(&operation.body().unwrap()).unwrap()
    }

    fn variables<D>(operation: Operation<D>) -> Value {
        body(&operation)["variables"].take()
    }

    #[test]
    fn reads_the_kind_of_a_document() {
        for (document, kind) in [
            ("{ shop { name } }", OperationKind::Query),
            ("query Q { shop { name } }", OperationKind::Query),
            ("  # comment\n mutation { a }", OperationKind::Mutation),
            ("\u{feff}mutation M($x: ID!) { a }", OperationKind::Mutation),
            (
                "fragment F on Shop { name \"}\" # }\n } , mutation { a { ...F } }",
                OperationKind::Mutation,
            ),
            (
                "fragment F on Shop { name } query { shop { ...F } }",
                OperationKind::Query,
            ),
            ("", OperationKind::Query),
        ] {
            assert_eq!(OperationKind::of(document), kind, "{document}");
        }
    }

    /// A comment ends at `\r`, `\n` or `\r\n` (the spec's LineTerminator).
    #[test]
    fn a_comment_ends_at_any_line_terminator() {
        for document in [
            "# comment\rmutation { tagsAdd(id: \"1\", tags: [\"a\"]) { userErrors { message } } }",
            "# comment\r\nmutation { a }",
            "# comment\nmutation { a }",
            "query { a # }\r } mutation { b }",
            "query { a } # x\rmutation { b }",
        ] {
            assert_eq!(
                OperationKind::of(document),
                OperationKind::Mutation,
                "{document:?}"
            );
        }
    }

    /// When the document can't be read, it's treated as a mutation: never retried.
    #[test]
    fn a_document_it_cannot_read_is_a_mutation() {
        for document in [
            "mutations",
            "subscription { a }",
            "query { a",
            "query { a(x: \"}) } mutation { b }",
            "query { a(x: \"\"\" } \" \"\"\") } mutation { b }",
            "query { a } }",
            "query @d(x: \"{\") { a } mutation { b }",
            "query ($x: In = {a: 1}) { a } mutation { b }",
        ] {
            assert_eq!(
                OperationKind::of(document),
                OperationKind::Mutation,
                "{document:?}"
            );
        }
        for document in [
            "query ($x: In = {a: 1}) { a(x: $x) }",
            "query { a(x: \"\"\" } \\\"\"\" \"\"\") }",
            "query @d(x: \"{\") { a }",
            "query { a } # mutation { b }\r\n",
        ] {
            assert_eq!(
                OperationKind::of(document),
                OperationKind::Query,
                "{document:?}"
            );
        }
    }

    #[test]
    fn converts_the_usual_spellings_of_a_value() {
        let operation = Operation::<()>::new("query { shop { id } }")
            .variable::<String, _>("id", "gid://shopify/Order/1")
            .variable::<Option<String>, _>("query", "status:open")
            .variable::<Option<String>, _>("after", None::<String>)
            .variable::<i64, _>("first", 10_u32)
            .variable::<i64, _>("count", 3_usize)
            .variable::<Option<i64>, _>("last", Some(2))
            .variable::<f64, _>("ratio", 1.5_f32)
            .variable::<bool, _>("reverse", true)
            .variable::<Vec<String>, _>("ids", ["a", "b"])
            .variable::<Vec<String>, _>("tags", &["x".to_string()][..])
            .variable::<Option<Vec<i64>>, _>("quantities", vec![1_u8, 2])
            .variable::<Vec<Option<String>>, _>("holes", vec![Some("a"), None])
            .variable::<Value, _>("payload", json!({ "a": 1 }))
            .json_variable("input", json!({ "title": "Shirt" }));

        assert_eq!(operation.query(), "query { shop { id } }");
        assert_eq!(
            variables(operation),
            json!({
                "id": "gid://shopify/Order/1",
                "query": "status:open",
                "after": null,
                "first": 10,
                "count": 3,
                "last": 2,
                "ratio": 1.5,
                "reverse": true,
                "ids": ["a", "b"],
                "tags": ["x"],
                "quantities": [1, 2],
                "holes": ["a", null],
                "payload": { "a": 1 },
                "input": { "title": "Shirt" },
            })
        );
    }

    #[test]
    fn values_shopify_would_not_read_as_written_are_refused_before_sending() {
        let operation = || Operation::<()>::new("query { shop { id } }");
        let fits = operation()
            .variable::<i64, _>("min", i32::MIN)
            .variable::<i64, _>("max", i64::from(i32::MAX))
            .variable::<i64, _>("unsigned", u64::from(i32::MAX.unsigned_abs()))
            .variable::<f64, _>("ratio", f64::MAX)
            .variable::<Option<Value>, _>("nullable", Value::Null)
            .variable::<Vec<Option<Value>>, _>("items", vec![Value::Null])
            .variable::<Value, _>("nested", json!({ "a": null }));
        assert_eq!(
            variables(fits),
            json!({
                "min": i32::MIN,
                "max": i32::MAX,
                "unsigned": i32::MAX,
                "ratio": f64::MAX,
                "nullable": null,
                "items": [null],
                "nested": { "a": null },
            })
        );

        for (operation, shown) in [
            (
                operation().variable::<i64, _>("big", u64::MAX),
                "invalid value at `$big`: Int out of 32-bit range",
            ),
            (
                operation().variable::<i64, _>("first", 2_147_483_648_i64),
                "invalid value at `$first`: Int out of 32-bit range",
            ),
            (
                operation().variable::<Option<i64>, _>("last", -2_147_483_649_i64),
                "invalid value at `$last`: Int out of 32-bit range",
            ),
            (
                operation().variable::<f64, _>("ratio", f64::NAN),
                "invalid value at `$ratio`: not a finite Float",
            ),
            (
                operation().variable::<Option<f64>, _>("ratio", f32::NEG_INFINITY),
                "invalid value at `$ratio`: not a finite Float",
            ),
            (
                operation().variable::<Vec<f64>, _>("ratios", [1.0, f64::INFINITY]),
                "invalid value at `$ratios[1]`: not a finite Float",
            ),
            (
                operation().variable::<Vec<Option<i64>>, _>(
                    "counts",
                    vec![Some(1), None, Some(1_i64 << 40)],
                ),
                "invalid value at `$counts[2]`: Int out of 32-bit range",
            ),
            (
                operation().variable::<Value, _>("settings", Value::Null),
                "invalid value at `$settings`: null for a non-null JSON field",
            ),
            (
                operation().variable::<Vec<Value>, _>("settings", vec![json!(1), Value::Null]),
                "invalid value at `$settings[1]`: null for a non-null JSON field",
            ),
            (
                operation().variable::<f64, _>("a name", f64::NAN),
                "invalid value at `$[variable]`: not a finite Float",
            ),
            (
                // The first refusal stays.
                operation()
                    .variable::<i64, _>("first", u32::MAX)
                    .variable::<f64, _>("ratio", f64::NAN)
                    .variable::<i64, _>("first", 1),
                "invalid value at `$first`: Int out of 32-bit range",
            ),
        ] {
            let error = operation.body().unwrap_err();
            assert_eq!(error.to_string(), shown);
            assert!(matches!(error, Error::InvalidValue { detail: None, .. }));
            assert_eq!(
                operation.variable_json("first").unwrap_err().to_string(),
                shown
            );
        }
    }

    /// The text sent, not parsed back: no depth limit, no stack used per
    /// level, no float rounded.
    #[test]
    fn the_body_and_the_variables_are_the_text_sent() {
        let mut value = json!(0);
        for _ in 0..250 {
            value = json!([value]);
        }
        let deep = format!("{}0{}", "[".repeat(250), "]".repeat(250));
        let float = 2.3318716180463287e-130_f64;
        let operation = Operation::<()>::new("query { shop { id } }")
            .json_variable("deep", &value)
            .variable::<f64, _>("x", float);
        let body = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || operation.body().map(|body| (body, operation)))
            .unwrap()
            .join()
            .unwrap();
        let (body, operation) = body.unwrap();
        assert_eq!(
            body,
            format!(
                r#"{{"query":"query {{ shop {{ id }} }}","variables":{{"deep":{deep},"x":2.3318716180463287e-130}}}}"#
            )
        );
        assert_eq!(
            operation.variable_json("deep").unwrap(),
            Some(deep.as_str())
        );
        assert_eq!(
            operation.variable_json("x").unwrap(),
            Some("2.3318716180463287e-130")
        );
        assert_eq!(operation.variable_json("y").unwrap(), None);
    }

    #[test]
    fn variables_can_come_from_a_struct() {
        #[derive(serde::Serialize)]
        struct Variables {
            first: i64,
            #[serde(skip_serializing_if = "Option::is_none")]
            after: Option<String>,
        }
        let operation = Operation::<()>::new("query { shop { id } }")
            .variable::<i64, _>("ignored", 1)
            .variables(Variables {
                first: 5,
                after: None,
            });
        assert_eq!(variables(operation), json!({ "first": 5 }));

        let error = Operation::<()>::new("query { shop { id } }")
            .variables(vec![1])
            .body()
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "invalid variables: the variables must serialize to an object"
        );
    }

    #[test]
    fn a_value_that_does_not_serialize_is_reported_when_the_operation_runs() {
        struct Broken;
        impl Serialize for Broken {
            fn serialize<S: serde::Serializer>(
                &self,
                _serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                Err(serde::ser::Error::custom("no"))
            }
        }
        let error = Operation::<()>::new("query { shop { id } }")
            .json_variable("input", Broken)
            .variable::<i64, _>("first", 1)
            .body()
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "invalid variables: variable `$input` does not serialize: a value's `Serialize` implementation failed"
        );
        assert_eq!(error.sensitive_detail(), Some("no"));
    }

    #[test]
    fn the_body_holds_the_query_its_variables_and_its_name() {
        let operation = Operation::<()>::__checked(
            "query Shop { shop { id } }",
            OperationKind::Query,
            Some("Shop"),
            "2026-07",
        );
        assert_eq!(
            operation.body().unwrap(),
            r#"{"query":"query Shop { shop { id } }","operationName":"Shop"}"#
        );
        assert_eq!(operation.api_version().unwrap(), ApiVersion::V2026_07);
        let operation = operation.variable::<i64, _>("first", 1);
        assert_eq!(
            operation.body().unwrap(),
            r#"{"query":"query Shop { shop { id } }","variables":{"first":1},"operationName":"Shop"}"#
        );
        assert_eq!(
            Operation::<()>::new("{ shop { id } }")
                .api_version()
                .unwrap(),
            ApiVersion::DEFAULT
        );
    }
}
