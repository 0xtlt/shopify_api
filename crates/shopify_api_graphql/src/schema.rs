//! The part of a GraphQL schema that checking and typing an operation needs:
//! type kinds, fields, arguments, enum values and deprecations, without
//! descriptions.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The introspection query whose result [`Schema::from_introspection`] reads.
///
/// It asks for deprecated fields, arguments, input fields and enum values
/// too: an operation may still use them, and `graphql!` warns when it does.
pub const INTROSPECTION_QUERY: &str = "\
query IntrospectionQuery {
  __schema {
    queryType { name }
    mutationType { name }
    types {
      kind
      name
      fields(includeDeprecated: true) {
        name
        args(includeDeprecated: true) { ...InputValue }
        type { ...TypeRef }
        isDeprecated
        deprecationReason
      }
      inputFields(includeDeprecated: true) { ...InputValue }
      interfaces { name }
      enumValues(includeDeprecated: true) { name isDeprecated deprecationReason }
      possibleTypes { name }
    }
    directives {
      name
      locations
      isRepeatable
      args(includeDeprecated: true) { ...InputValue }
    }
  }
}
fragment InputValue on __InputValue {
  name
  type { ...TypeRef }
  defaultValue
  isDeprecated
  deprecationReason
}
fragment TypeRef on __Type {
  kind name
  ofType { kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name } } } } } }
}
";

/// The version of the stored form written by [`Schema::to_json`].
pub(crate) const FORMAT: u32 = 1;

/// The reason GraphQL gives when a deprecation has none.
const NO_REASON: &str = "No longer supported";

/// A reference to a type, as written in GraphQL: `[Order!]!`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TypeRef {
    Named(String),
    List(Box<TypeRef>),
    NonNull(Box<TypeRef>),
}

impl TypeRef {
    /// Parses `[Order!]!`. Returns `None` for malformed input.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if let Some(inner) = text.strip_suffix('!') {
            return Some(Self::NonNull(Box::new(Self::parse(inner)?)));
        }
        if let Some(inner) = text.strip_prefix('[') {
            return Some(Self::List(Box::new(Self::parse(inner.strip_suffix(']')?)?)));
        }
        let is_name = !text.is_empty()
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        is_name.then(|| Self::Named(text.to_string()))
    }

    /// The innermost named type: `Order` for `[Order!]!`.
    pub fn name(&self) -> &str {
        match self {
            Self::Named(name) => name,
            Self::List(inner) | Self::NonNull(inner) => inner.name(),
        }
    }

    pub fn is_non_null(&self) -> bool {
        matches!(self, Self::NonNull(_))
    }

    /// The same type without its outer `!`.
    pub fn nullable(&self) -> &TypeRef {
        match self {
            Self::NonNull(inner) => inner,
            other => other,
        }
    }
}

impl fmt::Display for TypeRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(name) => formatter.write_str(name),
            Self::List(inner) => write!(formatter, "[{inner}]"),
            Self::NonNull(inner) => write!(formatter, "{inner}!"),
        }
    }
}

/// An argument or an input field.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct InputValue {
    pub ty: TypeRef,
    /// The schema gives it a default value.
    pub has_default: bool,
    /// Why it is deprecated, when it is.
    pub deprecation: Option<String>,
}

impl InputValue {
    /// A value of type `ty`, without default and not deprecated.
    pub fn of(ty: TypeRef) -> Self {
        Self {
            ty,
            has_default: false,
            deprecation: None,
        }
    }

    /// A value must be provided: the type is non-null and has no default.
    pub fn is_required(&self) -> bool {
        self.ty.is_non_null() && !self.has_default
    }
}

/// The places of an operation where a directive may be written.
pub const EXECUTABLE_LOCATIONS: [&str; 8] = [
    "QUERY",
    "MUTATION",
    "SUBSCRIPTION",
    "FIELD",
    "FRAGMENT_DEFINITION",
    "FRAGMENT_SPREAD",
    "INLINE_FRAGMENT",
    "VARIABLE_DEFINITION",
];

/// A directive an operation may use, such as `@include` or `@idempotent`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Directive {
    /// Where it may be written: [`EXECUTABLE_LOCATIONS`] only.
    pub locations: Vec<String>,
    pub args: BTreeMap<String, InputValue>,
    /// It may be written more than once at the same place.
    pub repeatable: bool,
}

impl Directive {
    /// `@include` and `@skip`, which every GraphQL schema has: what a schema
    /// stored without its directives gets.
    fn conditions() -> BTreeMap<String, Directive> {
        let condition = Directive {
            locations: ["FIELD", "FRAGMENT_SPREAD", "INLINE_FRAGMENT"]
                .map(str::to_string)
                .to_vec(),
            args: [(
                "if".to_string(),
                InputValue::of(TypeRef::NonNull(Box::new(TypeRef::Named(
                    "Boolean".to_string(),
                )))),
            )]
            .into(),
            repeatable: false,
        };
        [
            ("include".to_string(), condition.clone()),
            ("skip".to_string(), condition),
        ]
        .into()
    }
}

/// A field of an object or an interface.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Field {
    pub ty: TypeRef,
    pub args: BTreeMap<String, InputValue>,
    /// Why it is deprecated, when it is.
    pub deprecation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TypeDef {
    Scalar,
    Enum {
        values: Vec<String>,
        /// The deprecated values, with their reason.
        deprecated: BTreeMap<String, String>,
    },
    Object {
        fields: BTreeMap<String, Field>,
        interfaces: Vec<String>,
    },
    Interface {
        fields: BTreeMap<String, Field>,
        possible_types: Vec<String>,
    },
    Union {
        possible_types: Vec<String>,
    },
    Input {
        fields: BTreeMap<String, InputValue>,
    },
}

impl TypeDef {
    /// The kind of type, with its article, for error messages.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::Scalar => "a scalar",
            Self::Enum { .. } => "an enum",
            Self::Object { .. } => "an object",
            Self::Interface { .. } => "an interface",
            Self::Union { .. } => "a union",
            Self::Input { .. } => "an input object",
        }
    }

    /// Objects, interfaces and unions: types that need a selection of subfields.
    pub fn is_composite(&self) -> bool {
        matches!(
            self,
            Self::Object { .. } | Self::Interface { .. } | Self::Union { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Schema {
    /// The Shopify API version the schema was downloaded for, such as `2026-10`.
    pub api_version: String,
    pub query: String,
    pub mutation: Option<String>,
    pub types: BTreeMap<String, TypeDef>,
    /// The directives operations may use, by name.
    pub directives: BTreeMap<String, Directive>,
}

/// A schema file or an introspection result is malformed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaError(pub String);

impl fmt::Display for SchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for SchemaError {}

fn schema_error(message: impl Into<String>) -> SchemaError {
    SchemaError(message.into())
}

// ─── Stored form ─────────────────────────────────────────────────────
//
// One JSON object per type, with one-letter keys: the Admin schema has
// thousands of types, and it is parsed each time a crate using it compiles.
//
// A field is `"ID!"`, or `["OrderConnection!", { "first": "Int", "reverse": "Boolean=" }]`
// with its arguments. An input value is its type, followed by `=` when it has
// a default. Deprecations are kept apart, in `d`, by name: the reason of a
// field, of an enum value or of an input field, and `"field(argument)"` for
// an argument.

#[derive(Serialize, Deserialize)]
struct StoredSchema {
    format: u32,
    #[serde(rename = "apiVersion")]
    api_version: String,
    query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mutation: Option<String>,
    #[serde(default)]
    directives: BTreeMap<String, StoredDirective>,
    types: BTreeMap<String, StoredType>,
}

/// `{ "l": ["FIELD"], "a": { "key": "String!" } }`, with `"r": true` when repeatable.
#[derive(Serialize, Deserialize)]
struct StoredDirective {
    #[serde(rename = "l")]
    locations: Vec<String>,
    #[serde(rename = "a", default, skip_serializing_if = "BTreeMap::is_empty")]
    args: BTreeMap<String, String>,
    #[serde(rename = "r", default, skip_serializing_if = "std::ops::Not::not")]
    repeatable: bool,
}

type Deprecations = BTreeMap<String, String>;

#[derive(Serialize, Deserialize)]
enum StoredType {
    #[serde(rename = "s")]
    Scalar,
    #[serde(rename = "e")]
    Enum(StoredEnum),
    #[serde(rename = "o")]
    Object(StoredComposite),
    #[serde(rename = "n")]
    Interface(StoredComposite),
    #[serde(rename = "u")]
    Union(Vec<String>),
    #[serde(rename = "i")]
    Input(StoredInput),
}

/// `["PAID", "PENDING"]`, or `{ "v": […], "d": { "PENDING": "reason" } }`.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum StoredEnum {
    Plain(Vec<String>),
    WithDeprecations { v: Vec<String>, d: Deprecations },
}

/// `{ "id": "ID!" }`, or `{ "f": { … }, "d": { "id": "reason" } }`.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum StoredInput {
    WithDeprecations {
        f: BTreeMap<String, String>,
        d: Deprecations,
    },
    Plain(BTreeMap<String, String>),
}

#[derive(Serialize, Deserialize)]
struct StoredComposite {
    #[serde(rename = "f")]
    fields: BTreeMap<String, StoredField>,
    /// Interfaces of an object, possible types of an interface.
    #[serde(rename = "t", default, skip_serializing_if = "Vec::is_empty")]
    types: Vec<String>,
    #[serde(rename = "d", default, skip_serializing_if = "BTreeMap::is_empty")]
    deprecated: Deprecations,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
enum StoredField {
    Plain(String),
    WithArgs(String, BTreeMap<String, String>),
}

fn store_input(value: &InputValue) -> String {
    if value.has_default {
        format!("{}=", value.ty)
    } else {
        value.ty.to_string()
    }
}

fn load_input(
    owner: &str,
    name: &str,
    text: &str,
    deprecation: Option<&String>,
) -> Result<InputValue, SchemaError> {
    let (ty, has_default) = match text.strip_suffix('=') {
        Some(ty) => (ty, true),
        None => (text, false),
    };
    let ty = TypeRef::parse(ty)
        .ok_or_else(|| schema_error(format!("Invalid type \"{text}\" for {owner}.{name}")))?;
    Ok(InputValue {
        ty,
        has_default,
        deprecation: deprecation.cloned(),
    })
}

fn argument_key(field: &str, argument: &str) -> String {
    format!("{field}({argument})")
}

fn store_fields(fields: &BTreeMap<String, Field>) -> (BTreeMap<String, StoredField>, Deprecations) {
    let mut deprecated = Deprecations::new();
    let stored = fields
        .iter()
        .map(|(name, field)| {
            if let Some(reason) = &field.deprecation {
                deprecated.insert(name.clone(), reason.clone());
            }
            let ty = field.ty.to_string();
            let stored = if field.args.is_empty() {
                StoredField::Plain(ty)
            } else {
                let args = field
                    .args
                    .iter()
                    .map(|(argument, value)| {
                        if let Some(reason) = &value.deprecation {
                            deprecated.insert(argument_key(name, argument), reason.clone());
                        }
                        (argument.clone(), store_input(value))
                    })
                    .collect();
                StoredField::WithArgs(ty, args)
            };
            (name.clone(), stored)
        })
        .collect();
    (stored, deprecated)
}

fn load_fields(
    owner: &str,
    fields: BTreeMap<String, StoredField>,
    deprecated: &Deprecations,
) -> Result<BTreeMap<String, Field>, SchemaError> {
    fields
        .into_iter()
        .map(|(name, stored)| {
            let (ty, args) = match stored {
                StoredField::Plain(ty) => (ty, BTreeMap::new()),
                StoredField::WithArgs(ty, args) => (ty, args),
            };
            let field = Field {
                ty: TypeRef::parse(&ty).ok_or_else(|| {
                    schema_error(format!("Invalid type \"{ty}\" for {owner}.{name}"))
                })?,
                args: args
                    .into_iter()
                    .map(|(argument, text)| {
                        let reason = deprecated.get(&argument_key(&name, &argument));
                        let value =
                            load_input(&format!("{owner}.{name}"), &argument, &text, reason)?;
                        Ok((argument, value))
                    })
                    .collect::<Result<_, SchemaError>>()?,
                deprecation: deprecated.get(&name).cloned(),
            };
            Ok((name, field))
        })
        .collect()
}

fn load_inputs(
    owner: &str,
    fields: BTreeMap<String, String>,
    deprecated: &Deprecations,
) -> Result<BTreeMap<String, InputValue>, SchemaError> {
    fields
        .into_iter()
        .map(|(field, text)| {
            let value = load_input(owner, &field, &text, deprecated.get(&field))?;
            Ok((field, value))
        })
        .collect()
}

impl Schema {
    /// Reads the form written by [`Schema::to_json`].
    pub fn from_json(json: &str) -> Result<Self, SchemaError> {
        // Only `format`, without building the rest. Anything but an object
        // that reads takes the full parse, for its error.
        #[derive(Deserialize)]
        struct Header {
            format: Option<Value>,
        }
        let header = json
            .trim_start()
            .starts_with('{')
            .then(|| serde_json::from_str::<Header>(json).ok())
            .flatten();
        let format = match header {
            Some(header) => header.format,
            None => serde_json::from_str::<Value>(json)
                .map_err(|error| schema_error(format!("Invalid schema file: {error}")))?
                .get("format")
                .cloned(),
        };
        let format = format.as_ref().and_then(Value::as_u64);
        if format != Some(u64::from(FORMAT)) {
            return Err(schema_error(match format {
                Some(format) => format!(
                    "Unsupported schema file format {format}, this version reads format {FORMAT}"
                ),
                None => "Not a schema file of shopify_api: it has no `format`".to_string(),
            }));
        }
        let stored: StoredSchema = serde_json::from_str(json)
            .map_err(|error| schema_error(format!("Invalid schema file: {error}")))?;
        let types = stored
            .types
            .into_iter()
            .map(|(name, stored)| {
                let definition = match stored {
                    StoredType::Scalar => TypeDef::Scalar,
                    StoredType::Enum(StoredEnum::Plain(values)) => TypeDef::Enum {
                        values,
                        deprecated: Deprecations::new(),
                    },
                    StoredType::Enum(StoredEnum::WithDeprecations { v, d }) => TypeDef::Enum {
                        values: v,
                        deprecated: d,
                    },
                    StoredType::Object(object) => TypeDef::Object {
                        fields: load_fields(&name, object.fields, &object.deprecated)?,
                        interfaces: object.types,
                    },
                    StoredType::Interface(interface) => TypeDef::Interface {
                        fields: load_fields(&name, interface.fields, &interface.deprecated)?,
                        possible_types: interface.types,
                    },
                    StoredType::Union(possible_types) => TypeDef::Union { possible_types },
                    StoredType::Input(StoredInput::Plain(fields)) => TypeDef::Input {
                        fields: load_inputs(&name, fields, &Deprecations::new())?,
                    },
                    StoredType::Input(StoredInput::WithDeprecations { f, d }) => TypeDef::Input {
                        fields: load_inputs(&name, f, &d)?,
                    },
                };
                Ok((name, definition))
            })
            .collect::<Result<_, SchemaError>>()?;
        let mut directives: BTreeMap<String, Directive> = stored
            .directives
            .into_iter()
            .map(|(name, stored)| {
                let owner = format!("@{name}");
                let args = stored
                    .args
                    .into_iter()
                    .map(|(argument, text)| {
                        let value = load_input(&owner, &argument, &text, None)?;
                        Ok((argument, value))
                    })
                    .collect::<Result<_, SchemaError>>()?;
                let directive = Directive {
                    locations: stored.locations,
                    args,
                    repeatable: stored.repeatable,
                };
                Ok((name, directive))
            })
            .collect::<Result<_, SchemaError>>()?;
        if directives.is_empty() {
            directives = Directive::conditions();
        }
        Ok(Self {
            api_version: stored.api_version,
            query: stored.query,
            mutation: stored.mutation,
            types,
            directives,
        })
    }

    /// The stored form: one type per line, so that a new API version gives a readable diff.
    pub fn to_json(&self) -> String {
        let types: BTreeMap<&String, StoredType> = self
            .types
            .iter()
            .map(|(name, definition)| {
                let stored = match definition {
                    TypeDef::Scalar => StoredType::Scalar,
                    TypeDef::Enum { values, deprecated } if deprecated.is_empty() => {
                        StoredType::Enum(StoredEnum::Plain(values.clone()))
                    }
                    TypeDef::Enum { values, deprecated } => {
                        StoredType::Enum(StoredEnum::WithDeprecations {
                            v: values.clone(),
                            d: deprecated.clone(),
                        })
                    }
                    TypeDef::Object { fields, interfaces } => {
                        let (fields, deprecated) = store_fields(fields);
                        StoredType::Object(StoredComposite {
                            fields,
                            types: interfaces.clone(),
                            deprecated,
                        })
                    }
                    TypeDef::Interface {
                        fields,
                        possible_types,
                    } => {
                        let (fields, deprecated) = store_fields(fields);
                        StoredType::Interface(StoredComposite {
                            fields,
                            types: possible_types.clone(),
                            deprecated,
                        })
                    }
                    TypeDef::Union { possible_types } => StoredType::Union(possible_types.clone()),
                    TypeDef::Input { fields } => {
                        let stored = fields
                            .iter()
                            .map(|(name, value)| (name.clone(), store_input(value)))
                            .collect();
                        let deprecated: Deprecations = fields
                            .iter()
                            .filter_map(|(name, value)| {
                                Some((name.clone(), value.deprecation.clone()?))
                            })
                            .collect();
                        StoredType::Input(if deprecated.is_empty() {
                            StoredInput::Plain(stored)
                        } else {
                            StoredInput::WithDeprecations {
                                f: stored,
                                d: deprecated,
                            }
                        })
                    }
                };
                (name, stored)
            })
            .collect();

        let quoted = |text: &str| Value::from(text).to_string();
        let mut json = format!(
            "{{\"format\":{FORMAT},\"apiVersion\":{},\"query\":{}",
            quoted(&self.api_version),
            quoted(&self.query)
        );
        if let Some(mutation) = &self.mutation {
            json.push_str(&format!(",\"mutation\":{}", quoted(mutation)));
        }
        let directives: BTreeMap<&String, StoredDirective> = self
            .directives
            .iter()
            .map(|(name, directive)| {
                let stored = StoredDirective {
                    locations: directive.locations.clone(),
                    args: directive
                        .args
                        .iter()
                        .map(|(argument, value)| (argument.clone(), store_input(value)))
                        .collect(),
                    repeatable: directive.repeatable,
                };
                (name, stored)
            })
            .collect();
        let directives = serde_json::to_string(&directives).expect("directives serialize");
        json.push_str(&format!(",\n\"directives\":{directives}"));
        json.push_str(",\"types\":{");
        for (index, (name, stored)) in types.iter().enumerate() {
            json.push_str(if index == 0 { "\n" } else { ",\n" });
            let stored = serde_json::to_string(stored).expect("schema types serialize");
            json.push_str(&format!("{}:{stored}", quoted(name)));
        }
        json.push_str("\n}}\n");
        json
    }

    /// Builds a schema from the answer to [`INTROSPECTION_QUERY`]: the whole
    /// response, its `data`, or the `__schema` object.
    pub fn from_introspection(response: &Value, api_version: &str) -> Result<Self, SchemaError> {
        if let Some(errors) = response.get("errors").and_then(Value::as_array)
            && !errors.is_empty()
        {
            let messages: Vec<&str> = errors
                .iter()
                .filter_map(|error| error.get("message").and_then(Value::as_str))
                .collect();
            return Err(schema_error(format!(
                "Introspection failed: {}",
                messages.join("; ")
            )));
        }
        let schema = response
            .pointer("/data/__schema")
            .or_else(|| response.get("__schema"))
            .unwrap_or(response);
        let name_of = |pointer: &str| {
            schema
                .pointer(pointer)
                .and_then(Value::as_str)
                .map(str::to_string)
        };
        let query = name_of("/queryType/name")
            .ok_or_else(|| schema_error("The introspection result has no `queryType`"))?;
        let listed = schema
            .get("types")
            .and_then(Value::as_array)
            .ok_or_else(|| schema_error("The introspection result has no `types`"))?;

        let mut types = BTreeMap::new();
        for entry in listed {
            let name = text(entry, "name")?;
            if name.starts_with("__") {
                continue;
            }
            let names = |key: &str| -> Vec<String> {
                let listed = entry.get(key).and_then(Value::as_array);
                listed
                    .into_iter()
                    .flatten()
                    .filter_map(|item| item.get("name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            };
            let definition = match text(entry, "kind")? {
                "SCALAR" => TypeDef::Scalar,
                "ENUM" => {
                    let values = entry.get("enumValues").and_then(Value::as_array);
                    let deprecated = values
                        .into_iter()
                        .flatten()
                        .filter_map(|value| {
                            Some((
                                value.get("name")?.as_str()?.to_string(),
                                deprecation(value)?,
                            ))
                        })
                        .collect();
                    TypeDef::Enum {
                        values: names("enumValues"),
                        deprecated,
                    }
                }
                "OBJECT" => TypeDef::Object {
                    fields: introspected_fields(entry)?,
                    interfaces: names("interfaces"),
                },
                "INTERFACE" => TypeDef::Interface {
                    fields: introspected_fields(entry)?,
                    possible_types: names("possibleTypes"),
                },
                "UNION" => TypeDef::Union {
                    possible_types: names("possibleTypes"),
                },
                "INPUT_OBJECT" => TypeDef::Input {
                    fields: introspected_inputs(entry.get("inputFields"))?,
                },
                other => {
                    return Err(schema_error(format!(
                        "Unknown kind \"{other}\" for type {name}"
                    )));
                }
            };
            types.insert(name.to_string(), definition);
        }

        if !matches!(types.get(&query), Some(TypeDef::Object { .. })) {
            return Err(schema_error(format!(
                "The query type `{query}` is not an object of the schema"
            )));
        }

        let mut directives = BTreeMap::new();
        for entry in schema
            .get("directives")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let locations: Vec<String> = entry
                .get("locations")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|location| EXECUTABLE_LOCATIONS.contains(location))
                .map(str::to_string)
                .collect();
            if locations.is_empty() {
                continue;
            }
            let directive = Directive {
                locations,
                args: introspected_inputs(entry.get("args"))?,
                repeatable: entry.get("isRepeatable").and_then(Value::as_bool) == Some(true),
            };
            directives.insert(text(entry, "name")?.to_string(), directive);
        }
        if directives.is_empty() {
            directives = Directive::conditions();
        }

        Ok(Self {
            api_version: api_version.to_string(),
            query,
            mutation: name_of("/mutationType/name"),
            types,
            directives,
        })
    }

    pub fn get(&self, name: &str) -> Option<&TypeDef> {
        self.types.get(name)
    }

    pub fn directive(&self, name: &str) -> Option<&Directive> {
        self.directives.get(name)
    }

    /// The object types a value of `name` can be at run time.
    pub fn possible_types(&self, name: &str) -> Vec<&str> {
        match self.types.get_key_value(name) {
            Some((name, TypeDef::Object { .. })) => vec![name.as_str()],
            Some((
                _,
                TypeDef::Interface { possible_types, .. } | TypeDef::Union { possible_types },
            )) => possible_types.iter().map(String::as_str).collect(),
            _ => Vec::new(),
        }
    }
}

fn text<'a>(entry: &'a Value, key: &str) -> Result<&'a str, SchemaError> {
    entry
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| schema_error(format!("An introspected entry has no `{key}`")))
}

/// The reason of a deprecated entry, `None` when it is not deprecated.
fn deprecation(entry: &Value) -> Option<String> {
    if entry.get("isDeprecated").and_then(Value::as_bool) != Some(true) {
        return None;
    }
    let reason = entry.get("deprecationReason").and_then(Value::as_str);
    Some(
        reason
            .filter(|reason| !reason.trim().is_empty())
            .unwrap_or(NO_REASON)
            .trim()
            .to_string(),
    )
}

fn introspected_type(value: Option<&Value>) -> Result<TypeRef, SchemaError> {
    let value = value.ok_or_else(|| schema_error("An introspected entry has no `type`"))?;
    let inner = || introspected_type(value.get("ofType").filter(|inner| !inner.is_null()));
    Ok(match text(value, "kind")? {
        "NON_NULL" => TypeRef::NonNull(Box::new(inner()?)),
        "LIST" => TypeRef::List(Box::new(inner()?)),
        _ => TypeRef::Named(text(value, "name")?.to_string()),
    })
}

fn introspected_inputs(
    values: Option<&Value>,
) -> Result<BTreeMap<String, InputValue>, SchemaError> {
    values
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|value| {
            let input = InputValue {
                ty: introspected_type(value.get("type"))?,
                has_default: value
                    .get("defaultValue")
                    .is_some_and(|default| !default.is_null()),
                deprecation: deprecation(value),
            };
            Ok((text(value, "name")?.to_string(), input))
        })
        .collect()
}

fn introspected_fields(entry: &Value) -> Result<BTreeMap<String, Field>, SchemaError> {
    entry
        .get("fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|field| {
            let definition = Field {
                ty: introspected_type(field.get("type"))?,
                args: introspected_inputs(field.get("args"))?,
                deprecation: deprecation(field),
            };
            Ok((text(field, "name")?.to_string(), definition))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_and_prints_type_references() {
        for text in ["Order", "Order!", "[Order]", "[Order!]!", "[[Int!]]!"] {
            assert_eq!(TypeRef::parse(text).unwrap().to_string(), text);
        }
        let list = TypeRef::parse("[Order!]!").unwrap();
        assert_eq!(list.name(), "Order");
        assert!(list.is_non_null());
        assert_eq!(list.nullable().to_string(), "[Order!]");
        for invalid in ["", "!", "[Order", "Order]", "Or der", "[]"] {
            assert_eq!(TypeRef::parse(invalid), None, "{invalid}");
        }
    }

    fn introspection() -> Value {
        let named = |kind: &str, name: &str| json!({ "kind": kind, "name": name, "ofType": null });
        let non_null = |inner: Value| json!({ "kind": "NON_NULL", "name": null, "ofType": inner });
        let list = |inner: Value| json!({ "kind": "LIST", "name": null, "ofType": inner });
        json!({ "data": { "__schema": {
            "queryType": { "name": "QueryRoot" },
            "mutationType": { "name": "Mutation" },
            "types": [
                { "kind": "OBJECT", "name": "QueryRoot", "interfaces": [], "fields": [{
                    "name": "orders",
                    "type": non_null(list(non_null(named("OBJECT", "Order")))),
                    "isDeprecated": false,
                    "deprecationReason": null,
                    "args": [
                        { "name": "first", "type": named("SCALAR", "Int"), "defaultValue": null },
                        { "name": "reverse", "type": named("SCALAR", "Boolean"), "defaultValue": "false" },
                        {
                            "name": "savedSearchId",
                            "type": named("SCALAR", "ID"),
                            "defaultValue": null,
                            "isDeprecated": true,
                            "deprecationReason": "Use `query` instead.",
                        },
                    ],
                }] },
                { "kind": "OBJECT", "name": "Mutation", "interfaces": [], "fields": [] },
                { "kind": "OBJECT", "name": "Order", "interfaces": [{ "name": "Node" }], "fields": [
                    { "name": "id", "type": non_null(named("SCALAR", "ID")), "args": [] },
                    {
                        "name": "legacyName",
                        "type": named("SCALAR", "String"),
                        "args": [],
                        "isDeprecated": true,
                        "deprecationReason": null,
                    },
                ] },
                { "kind": "INTERFACE", "name": "Node", "possibleTypes": [{ "name": "Order" }], "fields": [
                    { "name": "id", "type": non_null(named("SCALAR", "ID")), "args": [] },
                ] },
                { "kind": "UNION", "name": "Resource", "possibleTypes": [{ "name": "Order" }] },
                { "kind": "ENUM", "name": "SortKey", "enumValues": [
                    { "name": "ID", "isDeprecated": false },
                    { "name": "CREATED_AT", "isDeprecated": true, "deprecationReason": "Use `ID`." },
                ] },
                { "kind": "INPUT_OBJECT", "name": "OrderInput", "inputFields": [
                    { "name": "id", "type": non_null(named("SCALAR", "ID")), "defaultValue": null },
                    { "name": "tags", "type": list(non_null(named("SCALAR", "String"))), "defaultValue": "[]" },
                    {
                        "name": "note",
                        "type": named("SCALAR", "String"),
                        "defaultValue": null,
                        "isDeprecated": true,
                        "deprecationReason": "Use `notes`.",
                    },
                ] },
                { "kind": "SCALAR", "name": "ID" },
                { "kind": "OBJECT", "name": "__Type", "interfaces": [], "fields": [] },
            ],
        } } })
    }

    #[test]
    fn reads_an_introspection_result() {
        let schema = Schema::from_introspection(&introspection(), "2026-10").unwrap();

        assert_eq!(schema.api_version, "2026-10");
        assert_eq!(schema.query, "QueryRoot");
        assert_eq!(schema.mutation.as_deref(), Some("Mutation"));
        assert!(schema.get("__Type").is_none());

        let Some(TypeDef::Object { fields, .. }) = schema.get("QueryRoot") else {
            panic!("QueryRoot is an object");
        };
        let orders = &fields["orders"];
        assert_eq!(orders.ty.to_string(), "[Order!]!");
        assert!(!orders.args["first"].has_default);
        assert!(orders.args["reverse"].has_default);
        assert_eq!(
            orders.args["savedSearchId"].deprecation.as_deref(),
            Some("Use `query` instead.")
        );
        assert_eq!(orders.deprecation, None);

        let Some(TypeDef::Object { fields, .. }) = schema.get("Order") else {
            panic!("Order is an object");
        };
        // A deprecation without a reason gets the one GraphQL gives.
        assert_eq!(
            fields["legacyName"].deprecation.as_deref(),
            Some("No longer supported")
        );

        let Some(TypeDef::Input { fields }) = schema.get("OrderInput") else {
            panic!("OrderInput is an input object");
        };
        assert!(fields["id"].is_required());
        assert!(!fields["tags"].is_required());
        assert_eq!(fields["note"].deprecation.as_deref(), Some("Use `notes`."));

        assert_eq!(schema.possible_types("Node"), ["Order"]);
        assert_eq!(schema.possible_types("Resource"), ["Order"]);
        assert_eq!(schema.possible_types("Order"), ["Order"]);
        assert!(schema.possible_types("SortKey").is_empty());
        assert_eq!(
            schema.get("SortKey"),
            Some(&TypeDef::Enum {
                values: vec!["ID".to_string(), "CREATED_AT".to_string()],
                deprecated: [("CREATED_AT".to_string(), "Use `ID`.".to_string())].into(),
            })
        );
    }

    #[test]
    fn the_stored_form_round_trips() {
        let schema = Schema::from_introspection(&introspection(), "2026-10").unwrap();
        let json = schema.to_json();

        assert_eq!(Schema::from_json(&json).unwrap(), schema);
        assert!(json.starts_with("{\"format\":1,\"apiVersion\":\"2026-10\","));
        // One type per line, in name order.
        assert!(json.contains(
            "\n\"Order\":{\"o\":{\"f\":{\"id\":\"ID!\",\"legacyName\":\"String\"},\"t\":[\"Node\"],\"d\":{\"legacyName\":\"No longer supported\"}}},\n"
        ));
        assert!(json.contains("\"orders\":[\"[Order!]!\",{\"first\":\"Int\",\"reverse\":\"Boolean=\",\"savedSearchId\":\"ID\"}]"));
        assert!(json.contains("\"d\":{\"orders(savedSearchId)\":\"Use `query` instead.\"}"));
        assert!(json.contains("\n\"ID\":\"s\",\n"));
        // The last type ends the object.
        assert!(json.ends_with("\n\"SortKey\":{\"e\":{\"v\":[\"ID\",\"CREATED_AT\"],\"d\":{\"CREATED_AT\":\"Use `ID`.\"}}}\n}}\n"));
        assert!(json.contains("\"OrderInput\":{\"i\":{\"f\":{\"id\":\"ID!\",\"note\":\"String\",\"tags\":\"[String!]=\"},\"d\":{\"note\":\"Use `notes`.\"}}}"));
    }

    #[test]
    fn keeps_the_directives_an_operation_may_use() {
        let mut response = introspection();
        response["data"]["__schema"]["directives"] = json!([
            {
                "name": "idempotent",
                "locations": ["FIELD"],
                "isRepeatable": false,
                "args": [{ "name": "key", "type": { "kind": "NON_NULL", "name": null, "ofType": { "kind": "SCALAR", "name": "String", "ofType": null } }, "defaultValue": null }],
            },
            { "name": "deprecated", "locations": ["FIELD_DEFINITION", "ENUM_VALUE"], "isRepeatable": false, "args": [] },
            { "name": "tag", "locations": ["FIELD", "OBJECT"], "isRepeatable": true, "args": [] },
        ]);
        let schema = Schema::from_introspection(&response, "2026-10").unwrap();

        // A directive of the schema's definitions is left out; only executable places are kept.
        assert!(schema.directive("deprecated").is_none());
        assert_eq!(schema.directive("tag").unwrap().locations, ["FIELD"]);
        assert!(schema.directive("tag").unwrap().repeatable);
        assert!(schema.directive("idempotent").unwrap().args["key"].is_required());
        // Without `@include` and `@skip` in the answer, none are added.
        assert!(schema.directive("include").is_none());

        let json = schema.to_json();
        assert!(json.contains("\n\"directives\":{\"idempotent\":{\"l\":[\"FIELD\"],\"a\":{\"key\":\"String!\"}},\"tag\":{\"l\":[\"FIELD\"],\"r\":true}},\"types\":{"));
        assert_eq!(Schema::from_json(&json).unwrap(), schema);

        // A schema without directives has `@include` and `@skip`.
        let bare = Schema::from_introspection(&introspection(), "2026-10").unwrap();
        assert_eq!(
            bare.directives.keys().collect::<Vec<_>>(),
            ["include", "skip"]
        );
        assert!(admin().directive("idempotent").is_some());
    }

    #[test]
    fn reports_malformed_input() {
        assert_eq!(
            Schema::from_introspection(
                &json!({ "errors": [{ "message": "Throttled" }] }),
                "2026-10"
            ),
            Err(SchemaError("Introspection failed: Throttled".to_string()))
        );
        assert_eq!(
            Schema::from_introspection(&json!({ "data": { "__schema": {} } }), "2026-10"),
            Err(SchemaError(
                "The introspection result has no `queryType`".to_string()
            ))
        );
        assert_eq!(
            Schema::from_introspection(
                &json!({ "__schema": { "queryType": { "name": "Missing" }, "types": [] } }),
                "2026-10"
            ),
            Err(SchemaError(
                "The query type `Missing` is not an object of the schema".to_string()
            ))
        );
        assert!(
            Schema::from_json("{")
                .unwrap_err()
                .0
                .starts_with("Invalid schema file: ")
        );
        assert_eq!(
            Schema::from_json(r#"{"apiVersion":"2026-10","query":"Q","types":{}}"#),
            Err(SchemaError(
                "Not a schema file of shopify_api: it has no `format`".to_string()
            ))
        );
        assert_eq!(
            Schema::from_json(r#"[1]"#),
            Err(SchemaError(
                "Not a schema file of shopify_api: it has no `format`".to_string()
            ))
        );
        assert_eq!(
            Schema::from_json(r#"{"format":9,"apiVersion":"2026-10","query":"Q","types":{}}"#),
            Err(SchemaError(
                "Unsupported schema file format 9, this version reads format 1".to_string()
            ))
        );
        assert_eq!(
            Schema::from_json(
                r#"{"format":1,"apiVersion":"2026-10","query":"Q","types":{"Q":{"o":{"f":{"a":"[Int"}}}}}"#
            ),
            Err(SchemaError("Invalid type \"[Int\" for Q.a".to_string()))
        );
    }

    /// The Admin schema 2026-10 the tests of the workspace use.
    fn admin() -> Schema {
        Schema::from_json(include_str!("../../shopify_api/graphql/admin.schema.json")).unwrap()
    }

    #[test]
    fn the_test_schema_is_the_admin_api() {
        let schema = admin();
        assert_eq!(schema.api_version, "2026-10");
        assert_eq!(schema.query, "QueryRoot");
        assert_eq!(schema.mutation.as_deref(), Some("Mutation"));
        assert!(schema.types.len() > 1000);
    }
}
