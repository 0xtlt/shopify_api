//! Operations, as built by [`graphql!`](crate::graphql!) or by hand.

use std::borrow::Cow;
use std::marker::PhantomData;

use serde::Serialize;
use serde_json::{Map, Value};

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
    pub fn of(document: &str) -> Self {
        let mut rest = document;
        loop {
            rest = skip_ignored(rest);
            if rest.is_empty() {
                return Self::Query;
            }
            let word: String = rest
                .chars()
                .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
                .collect();
            if word == "mutation" {
                return Self::Mutation;
            }
            // A query, a fragment or a shorthand `{ … }`: on to the next definition.
            match skip_block(&rest[word.len()..]) {
                Some(after) => rest = after,
                None => return Self::Query,
            }
        }
    }

    pub fn is_mutation(&self) -> bool {
        matches!(self, Self::Mutation)
    }
}

/// Skips white space, commas, comments and a byte order mark.
fn skip_ignored(mut text: &str) -> &str {
    loop {
        let trimmed = text.trim_start_matches(|character: char| {
            character.is_whitespace() || character == ',' || character == '\u{feff}'
        });
        match trimmed.strip_prefix('#') {
            Some(comment) => text = comment.split_once('\n').map_or("", |(_, after)| after),
            None => return trimmed,
        }
    }
}

/// What follows the next `{ … }` block, strings and comments taken into account.
fn skip_block(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut chars = text[start..].char_indices().peekable();
    while let Some((index, character)) = chars.next() {
        match character {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start + index + 1..]);
                }
            }
            '#' => {
                for (_, character) in chars.by_ref() {
                    if character == '\n' {
                        break;
                    }
                }
            }
            '"' => {
                let mut escaped = false;
                for (_, character) in chars.by_ref() {
                    match character {
                        '\\' if !escaped => escaped = true,
                        '"' if !escaped => break,
                        _ => escaped = false,
                    }
                }
            }
            _ => {}
        }
    }
    None
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
    variables: std::result::Result<Map<String, Value>, String>,
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
            variables: Ok(Map::new()),
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
            variables: Ok(Map::new()),
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

    fn set(mut self, name: &str, value: serde_json::Result<Value>) -> Self {
        match (&mut self.variables, value) {
            (Ok(variables), Ok(value)) => {
                variables.insert(name.to_string(), value);
            }
            (Ok(_), Err(error)) => {
                self.variables = Err(format!("invalid value for variable `${name}`: {error}"))
            }
            (Err(_), _) => {}
        }
        self
    }

    /// Sets a variable to a value convertible to `T`, the Rust type of its
    /// GraphQL type.
    pub fn variable<T: Serialize, V: IntoVariable<T>>(self, name: &str, value: V) -> Self {
        self.set(name, serde_json::to_value(value.into_variable()))
    }

    /// Sets a variable to any serializable value, checked by nothing.
    pub fn json_variable<V: Serialize>(self, name: &str, value: V) -> Self {
        self.set(name, serde_json::to_value(value))
    }

    /// Replaces the variables with the fields of a serializable struct or map.
    pub fn variables<V: Serialize>(mut self, variables: V) -> Self {
        self.variables = match serde_json::to_value(variables) {
            Ok(Value::Object(variables)) => Ok(variables),
            Ok(Value::Null) => Ok(Map::new()),
            Ok(_) => Err("the variables of an operation must serialize to an object".to_string()),
            Err(error) => Err(format!("{error}")),
        };
        self
    }

    /// The variables, as they are sent.
    pub fn variables_json(&self) -> Result<&Map<String, Value>> {
        self.variables
            .as_ref()
            .map_err(|message| Error::Variables(message.clone()))
    }

    /// The body Shopify receives: `query`, `variables` and `operationName`.
    pub fn body(&self) -> Result<Value> {
        let mut body = Map::new();
        body.insert("query".to_string(), Value::String(self.query.to_string()));
        let variables = self.variables_json()?;
        if !variables.is_empty() {
            body.insert("variables".to_string(), Value::Object(variables.clone()));
        }
        if let Some(name) = &self.name {
            body.insert("operationName".to_string(), Value::String(name.to_string()));
        }
        Ok(Value::Object(body))
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
/// variable, a slice for a list.
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
    // Larger than any GraphQL `Int`: Shopify rejects the saturated value.
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

    fn variables<D>(operation: Operation<D>) -> Value {
        Value::Object(operation.variables_json().unwrap().clone())
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
            ("mutations", OperationKind::Query),
            ("", OperationKind::Query),
        ] {
            assert_eq!(OperationKind::of(document), kind, "{document}");
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
    fn integers_that_do_not_fit_are_saturated() {
        let operation = Operation::<()>::new("query { shop { id } }")
            .variable::<i64, _>("big", u64::MAX)
            .variable::<i64, _>("fits", 7_u64);
        assert_eq!(variables(operation), json!({ "big": i64::MAX, "fits": 7 }));
    }

    #[test]
    fn variables_can_come_from_a_struct() {
        #[derive(Serialize)]
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
            "invalid variables: the variables of an operation must serialize to an object"
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
            "invalid variables: invalid value for variable `$input`: no"
        );
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
            json!({ "query": "query Shop { shop { id } }", "operationName": "Shop" })
        );
        assert_eq!(operation.api_version().unwrap(), ApiVersion::V2026_07);
        let operation = operation.variable::<i64, _>("first", 1);
        assert_eq!(
            operation.body().unwrap(),
            json!({ "query": "query Shop { shop { id } }", "variables": { "first": 1 }, "operationName": "Shop" })
        );
        assert_eq!(
            Operation::<()>::new("{ shop { id } }")
                .api_version()
                .unwrap(),
            ApiVersion::DEFAULT
        );
    }
}
