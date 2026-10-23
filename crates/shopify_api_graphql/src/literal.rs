//! Values written in Rust for an input type: the variables of an inline
//! `graphql!`, and `input!`. The macros parse them into [`Value`]s;
//! [`Literals`] checks each one against the type expected where it stands
//! and writes the code that sends it.
//!
//! What a value means depends on that type:
//!
//! - `null` is `null`, where the type is nullable;
//! - `{ field: value, field?: option, … }` is an object where an input object
//!   is expected, every field checked;
//! - `[value, …]` and `[for pattern in iterable if condition => value]` are
//!   lists where a list is expected;
//! - a lone identifier is a value of the enum where an enum is expected,
//!   `raw(string)` a value nothing checks;
//! - anything else is a Rust expression, whose type rustc checks with a
//!   trait written for the place: it names the GraphQL type when it refuses
//!   one. An `Option` is refused in a field: `field?:` leaves the field out
//!   when it is `None`, `field: null` sends `null`.

use std::collections::BTreeMap;
use std::fmt;

use proc_macro2::{Delimiter, Group, Ident, Span, TokenStream, TokenTree};
use quote::{format_ident, quote, quote_spanned};

use crate::analyze::{Scalar, Variable, did_you_mean};
use crate::codegen::{deprecation_warning, input_object_id, variable_type};
use crate::schema::{InputValue, Schema, TypeDef, TypeRef};

/// A value, as the macros parse it.
#[derive(Debug, Clone)]
pub enum Value {
    /// `null`.
    Null(Span),
    /// `raw(value)`, `keyword` being the span of `raw`.
    Raw {
        keyword: Span,
        value: Expr,
    },
    /// A lone identifier: a value of the enum where an enum is expected, a
    /// Rust variable elsewhere.
    Ident(Ident),
    Object(Object),
    List(List),
    For(Box<For>),
    /// Any other Rust expression.
    Rust(Expr),
}

/// A Rust expression, and where to point at it.
#[derive(Debug, Clone)]
pub struct Expr {
    pub tokens: TokenStream,
    pub span: Span,
}

/// `{ field: value, … }`.
#[derive(Debug, Clone)]
pub struct Object {
    /// Of the braces.
    pub span: Span,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone)]
pub struct Field {
    /// The name of the GraphQL field, `type` included.
    pub name: String,
    pub name_span: Span,
    /// The span of the `?` of `field?: value`.
    pub optional: Option<Span>,
    pub value: Value,
}

/// `[value, …]`.
#[derive(Debug, Clone)]
pub struct List {
    /// Of the brackets.
    pub span: Span,
    pub items: Vec<Value>,
    /// The same text read as a Rust array expression, when it is one.
    pub rust: Option<Expr>,
}

/// `[for pattern in iterable if filter => item]`.
#[derive(Debug, Clone)]
pub struct For {
    /// Of the brackets.
    pub span: Span,
    pub pattern: TokenStream,
    pub iterable: TokenStream,
    pub filter: Option<TokenStream>,
    pub item: Value,
}

/// A value that does not fit the schema, and the token at fault.
#[derive(Debug, Clone)]
pub struct Error {
    pub span: Span,
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

fn error<T>(span: Span, message: impl Into<String>) -> Result<T, Error> {
    Err(Error {
        span,
        message: message.into(),
    })
}

/// How an inline variable is set on the operation.
#[derive(Debug, Clone)]
pub enum VariableValue {
    /// A Rust expression converted to `ty` by `IntoVariable`, as
    /// `Operation::variable::<ty, _>` does.
    Converted { ty: TokenStream, value: Expr },
    /// An expression of the JSON of the value, for
    /// `Operation::__literal_variable`.
    Literal(TokenStream),
}

/// Where a value stands, for messages.
#[derive(Clone)]
enum Site {
    Variable(String),
    Field { name: String, parent: String },
    Item(Box<Site>),
}

impl fmt::Display for Site {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Variable(name) => write!(formatter, "`${name}`"),
            Self::Field { name, parent } => write!(formatter, "`{name}` of `{parent}`"),
            Self::Item(site) => write!(formatter, "an item of {site}"),
        }
    }
}

/// What a value of a type may be: an `Option` stands for a nullable
/// variable or list item, not for a field.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Place {
    Variable,
    Field,
    Item,
}

/// Checks values against a schema and writes their code.
///
/// No name the code declares is in scope where a Rust expression of a value
/// is evaluated, so that none captures or shadows a name of the caller's.
/// The Rust expressions of a value are the arguments of one method, which
/// builds the value; the method, and the traits that check each argument,
/// are declared in a block of their own, the receiver of the call. A
/// constant (`"text"`, `12`, `true`) is written in place, with nothing to
/// check. The code declares no local that a constant of the caller's could
/// match.
pub struct Literals<'a> {
    schema: &'a Schema,
    krate: TokenStream,
    items: Vec<TokenStream>,
    count: usize,
    /// Of the names of the traits, from the crate being compiled: rustc
    /// points at a trait of the same name in another crate otherwise.
    suffix: u32,
}

impl fmt::Debug for Literals<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Literals")
            .field("api_version", &self.schema.api_version)
            .field("items", &self.items.len())
            .finish()
    }
}

impl<'a> Literals<'a> {
    /// `krate` is the path of the `shopify_api` crate in the generated code.
    pub fn new(schema: &'a Schema, krate: TokenStream) -> Self {
        Self {
            schema,
            krate,
            items: Vec::new(),
            count: 0,
            suffix: crate_suffix(),
        }
    }

    /// The warnings the code written so far gives: unnamed constants.
    pub fn finish(self) -> TokenStream {
        let items = self.items;
        quote!(#(#items)*)
    }

    /// The value of an inline variable. A Rust expression for a scalar, a
    /// list of scalars, or `raw(…)` for an enum, converts as it always did
    /// (`IntoVariable`, an `Option` sending `null`); anything else is written
    /// as JSON when the operation is built.
    pub fn variable(&mut self, variable: &Variable, value: &Value) -> Result<VariableValue, Error> {
        let ty = &variable.ty;
        let site = Site::Variable(variable.name.clone());
        let scalar = matches!(self.schema.get(ty.name()), Some(TypeDef::Scalar));
        if scalar {
            match value {
                Value::Rust(expr) => self.int_literals(&expr.tokens, ty, &site)?,
                Value::List(List {
                    rust: Some(rust), ..
                }) => self.int_literals(&rust.tokens, ty, &site)?,
                _ => {}
            }
        }
        let rust_type = variable_type(variable, &self.krate);
        let convert = |value: Expr| {
            rust_type
                .clone()
                .map(|ty| VariableValue::Converted { ty, value })
        };
        let converted = match value {
            Value::Rust(expr) if scalar => convert(expr.clone()),
            Value::Ident(ident) if scalar => convert(ident_expr(ident)),
            Value::List(List {
                items,
                rust: Some(rust),
                ..
            }) if scalar
                && items
                    .iter()
                    .all(|item| matches!(item, Value::Rust(_) | Value::Ident(_))) =>
            {
                convert(rust.clone())
            }
            Value::Raw { keyword, value } => {
                self.enum_only(ty, *keyword, &site)?;
                convert(value.clone())
            }
            _ => None,
        };
        if let Some(converted) = converted {
            return Ok(converted);
        }
        let mut build = Build::default();
        let body = self.value(value, ty, &site, Place::Variable, &mut build)?;
        Ok(VariableValue::Literal(build.finish(body, &self.krate)))
    }

    /// `input!(name { … })`: an `InputObject` of the input object `name`.
    pub fn input_object(
        &mut self,
        name: &str,
        span: Span,
        object: &Object,
    ) -> Result<TokenStream, Error> {
        let schema = self.schema;
        let fields = match schema.get(name) {
            Some(TypeDef::Input { fields }) => fields,
            Some(other) => {
                return error(
                    span,
                    format!(
                        "`{name}` is {}, not an input object: `input!` builds the input objects of the schema",
                        other.kind_name()
                    ),
                );
            }
            None => {
                let inputs = schema
                    .types
                    .iter()
                    .filter(|(_, definition)| matches!(definition, TypeDef::Input { .. }))
                    .map(|(name, _)| name.as_str());
                return error(
                    span,
                    format!(
                        "The schema has no input object `{name}`{}",
                        did_you_mean(name, inputs)
                    ),
                );
            }
        };
        let mut build = Build::default();
        let body = self.object(name, fields, object, &mut build)?;
        let json = build.finish(body, &self.krate);
        let krate = &self.krate;
        let id = input_object_id(name);
        Ok(quote_spanned! {span=>
            #krate::InputObject::<#id>::__new(#name, #json)
        })
    }

    /// The code that builds a value of type `ty` from the arguments it adds
    /// to `build`.
    fn value(
        &mut self,
        value: &Value,
        ty: &TypeRef,
        site: &Site,
        place: Place,
        build: &mut Build,
    ) -> Result<TokenStream, Error> {
        let krate = self.krate.clone();
        let inner = ty.nullable();
        match value {
            Value::Null(span) => {
                if ty.is_non_null() {
                    return error(
                        *span,
                        format!("`null` is not allowed for {site}, of type `{ty}`"),
                    );
                }
                Ok(quote_spanned!(*span=> ()))
            }
            Value::Raw { keyword, value } => {
                self.enum_only(ty, *keyword, site)?;
                let tokens = argument(value);
                let arg = quote_spanned!(value.span=> #krate::__private::literal::raw(#tokens));
                Ok(build.argument(None, quote!(::std::string::String), arg))
            }
            Value::Ident(ident) => match self.enum_values(inner) {
                Some((name, values, deprecated)) => {
                    self.enum_value(ident, name, values, deprecated, site)
                }
                None => self.expression(&ident_expr(ident), ty, site, place, build),
            },
            Value::Object(object) => match self.input_fields(inner) {
                Some((name, fields)) => self.object(name, fields, object, build),
                None => error(
                    object.span,
                    format!("{site} expects {}, not an object", self.what(ty)),
                ),
            },
            Value::List(list) => match (inner, &list.rust) {
                (TypeRef::List(item), _) => {
                    let site = Site::Item(Box::new(site.clone()));
                    let mut items = Vec::new();
                    for value in &list.items {
                        items.push(self.value(value, item, &site, Place::Item, build)?);
                    }
                    let nested = items
                        .iter()
                        .rev()
                        .fold(quote!(()), |rest, item| quote!((#item, #rest)));
                    Ok(quote!(literal::List(#nested)))
                }
                (_, Some(rust)) => self.expression(rust, ty, site, place, build),
                (_, None) => error(
                    list.span,
                    format!("{site} expects {}, not a list", self.what(ty)),
                ),
            },
            Value::For(comprehension) => {
                let TypeRef::List(item) = inner else {
                    return error(
                        comprehension.span,
                        format!("{site} expects {}, not a list", self.what(ty)),
                    );
                };
                let site = Site::Item(Box::new(site.clone()));
                let mut each = Build::default();
                let value = self.value(&comprehension.item, item, &site, Place::Item, &mut each)?;
                let value = each.finish(value, &krate);
                let For {
                    pattern,
                    iterable,
                    filter,
                    ..
                } = &**comprehension;
                // A generated name may meet a user item only in a compile
                // error (a constant in a pattern, E0530), never in a silent
                // change of meaning: mixed-site, no user expression sees it.
                let items = Ident::new("__shopify_api_collected", Span::mixed_site());
                // Each item is written in its turn of the loop, so that it may
                // borrow what the loop gives.
                let push =
                    quote!(#krate::__private::literal::Collected::push(&mut #items, #value););
                let body = match filter {
                    Some(filter) => quote!(if #filter { #push }),
                    None => push,
                };
                let list = quote! {{
                    let mut #items = <#krate::__private::literal::Collected as ::core::default::Default>::default();
                    for #pattern in #iterable { #body }
                    #items
                }};
                Ok(build.argument(None, quote!(#krate::__private::literal::Collected), list))
            }
            Value::Rust(expr) => self.expression(expr, ty, site, place, build),
        }
    }

    /// The fields of an object literal, for the input object `name`.
    fn object(
        &mut self,
        name: &str,
        definitions: &BTreeMap<String, InputValue>,
        object: &Object,
        build: &mut Build,
    ) -> Result<TokenStream, Error> {
        let mut given: Vec<&str> = Vec::new();
        let mut fields = Vec::new();
        for field in &object.fields {
            let key = field.name.as_str();
            let Some(definition) = definitions.get(key) else {
                return error(
                    field.name_span,
                    format!(
                        "Input object `{name}` has no field `{key}`{}",
                        did_you_mean(key, definitions.keys().map(String::as_str))
                    ),
                );
            };
            if given.contains(&key) {
                return error(
                    field.name_span,
                    format!("The field `{key}` of `{name}` is given twice"),
                );
            }
            given.push(key);
            if let Some(reason) = &definition.deprecation {
                let note = format!(
                    "`{name}.{key}` is deprecated in the Shopify Admin API {}: {reason}",
                    self.schema.api_version
                );
                self.items.push(deprecation_warning(field.name_span, &note));
            }
            let site = Site::Field {
                name: key.to_string(),
                parent: name.to_string(),
            };
            let code = match field.optional {
                Some(question) => {
                    if definition.is_required() {
                        return error(
                            question,
                            format!(
                                "{site} is required (`{}`): it cannot be left out, so `?:` does not apply; write `{key}: …`",
                                definition.ty
                            ),
                        );
                    }
                    let value = self.optional(field, definition, &site, build)?;
                    quote!(literal::Optional(#key, #value))
                }
                None => {
                    let value =
                        self.value(&field.value, &definition.ty, &site, Place::Field, build)?;
                    quote!(literal::Field(#key, #value))
                }
            };
            fields.push(code);
        }
        let missing: Vec<String> = definitions
            .iter()
            .filter(|(key, definition)| definition.is_required() && !given.contains(&key.as_str()))
            .map(|(key, definition)| format!("`{key}: {}`", definition.ty))
            .collect();
        if !missing.is_empty() {
            let (fields, them) = if missing.len() == 1 {
                ("field", "it")
            } else {
                ("fields", "them")
            };
            return error(
                object.span,
                format!(
                    "Input object `{name}` requires the {fields} {}: add {them}",
                    join(&missing, "and")
                ),
            );
        }
        let nested = fields
            .iter()
            .rev()
            .fold(quote!(()), |rest, field| quote!((#field, #rest)));
        Ok(quote!(literal::Object(#nested)))
    }

    /// `field?: value`, the field being optional: `Maybe<V>` of the value.
    fn optional(
        &mut self,
        field: &Field,
        definition: &InputValue,
        site: &Site,
        build: &mut Build,
    ) -> Result<TokenStream, Error> {
        let krate = self.krate.clone();
        let key = &field.name;
        // Non-null with a default: absent is fine, `null` is not.
        let null_refused = definition.ty.is_non_null();
        let expr = match &field.value {
            Value::Raw { keyword, value } => {
                self.enum_only(&definition.ty, *keyword, site)?;
                let tokens = argument(value);
                let convert = if null_refused {
                    quote!(raw_option)
                } else {
                    quote!(raw_optional)
                };
                let arg =
                    quote_spanned!(value.span=> #krate::__private::literal::#convert(#tokens));
                return Ok(build.argument(None, quote!(#krate::Maybe<::std::string::String>), arg));
            }
            Value::Rust(expr) => expr.clone(),
            Value::Ident(ident) => ident_expr(ident),
            Value::Null(span) => {
                return error(*span, format!("`{key}?: null` is `{key}: null`"));
            }
            Value::Object(Object { span, .. }) | Value::List(List { span, .. }) => {
                return error(*span, literal_after_question(key));
            }
            Value::For(comprehension) => {
                return error(comprehension.span, literal_after_question(key));
            }
        };
        if self.is_enum(&definition.ty) {
            return error(
                expr.span,
                format!(
                    "{site} holds a value of the enum `{}`: for a string known at run time, write `{key}?: raw(…)`, with an `Option` of it",
                    definition.ty.name()
                ),
            );
        }

        self.int_literals(&expr.tokens, &definition.ty, site)?;
        let kind = self.kind(&definition.ty);
        let value_bound = self.expect(&definition.ty, Place::Field, site, &kind, build);
        let n = self.next();
        let bound = self.name("Optional", n);
        let what = escape(&self.what(&definition.ty));
        let (message, label, maybe_bound) = if null_refused {
            let no_null = self.name("NoNull", n);
            let refusal = escape(&format!(
                "{site} is non-null (`{}`): `{key}?:` takes an `Option`, not a `Maybe`, whose `Null` it refuses",
                definition.ty
            ));
            build.items.push(quote! {
                #[diagnostic::on_unimplemented(message = #refusal, label = "an `Option`: `None` leaves the field out")]
                trait #no_null {}
            });
            (
                format!("`{key}?:` takes an `Option` of {what}, not `{{Self}}`"),
                "`None` leaves the field out".to_string(),
                quote!(#value_bound + #no_null),
            )
        } else {
            (
                format!(
                    "`{key}?:` takes an `Option` or a `shopify_api::Maybe` of {what}, not `{{Self}}`"
                ),
                "`None` and `Maybe::Absent` leave the field out, `Maybe::Null` sends `null`"
                    .to_string(),
                quote!(#value_bound),
            )
        };
        // `OptionalValue` as a supertrait: one refusal, which names the field.
        build.items.push(quote! {
            #[diagnostic::on_unimplemented(message = #message, label = #label)]
            trait #bound: literal::OptionalValue<#kind> {}
            impl<V: #value_bound> #bound for ::core::option::Option<V> {}
            impl<V: #maybe_bound> #bound for #krate::Maybe<V> {}
        });
        let generic = format_ident!("V{n}");
        let value = build.argument(
            Some(quote!(#generic: #bound)),
            quote!(#generic),
            argument(&expr),
        );
        Ok(quote! {
            literal::optional::<#kind, _>(
                literal::OptionalValue::<#kind>::into_maybe(#value)
            )
        })
    }

    /// A Rust expression for a value of type `ty`, checked by a trait.
    fn expression(
        &mut self,
        expr: &Expr,
        ty: &TypeRef,
        site: &Site,
        place: Place,
        build: &mut Build,
    ) -> Result<TokenStream, Error> {
        if self.is_enum(ty) {
            let name = ty.name();
            let message = match ty.nullable() {
                TypeRef::List(_) => format!(
                    "{site} expects a list of values of the enum `{name}`: write them bare, `[{}, …]`, or `[for value in … => raw(value)]` for strings known at run time",
                    self.first_value(name)
                ),
                _ => format!(
                    "{site} expects a value of the enum `{name}`: write it bare, as `{}`, or `raw(…)` for a string known at run time",
                    self.first_value(name)
                ),
            };
            return error(expr.span, message);
        }
        self.int_literals(&expr.tokens, ty, site)?;
        let kind = self.kind(ty);
        if let Some(constant) = self.constant(expr, ty) {
            return Ok(constant);
        }
        let bound = self.expect(ty, place, site, &kind, build);
        let generic = format_ident!("V{}", self.next());
        let value = build.argument(
            Some(quote!(#generic: #bound)),
            quote!(#generic),
            argument(expr),
        );
        Ok(quote!(literal::leaf::<#kind, _>(#value)))
    }

    /// A literal that has the type `ty` whatever its context, written in
    /// place: a string for a string scalar, an integer for an `Int` (checked
    /// already) or a `Float`, a float for a `Float`, `true` or `false`.
    fn constant(&self, expr: &Expr, ty: &TypeRef) -> Option<TokenStream> {
        let TypeRef::Named(name) = ty.nullable() else {
            return None;
        };
        if !matches!(self.schema.get(name), Some(TypeDef::Scalar)) {
            return None;
        }
        let trees: Vec<TokenTree> = expr.tokens.clone().into_iter().collect();
        let literal = match trees.as_slice() {
            [TokenTree::Ident(ident)] if ident == "true" || ident == "false" => {
                return (Scalar::of(name) == Scalar::Boolean).then(|| expr.tokens.clone());
            }
            [TokenTree::Literal(literal)] => literal.to_string(),
            [TokenTree::Punct(minus), TokenTree::Literal(literal)] if minus.as_char() == '-' => {
                literal.to_string()
            }
            _ => return None,
        };
        let tokens = &expr.tokens;
        match (Scalar::of(name), number(&literal)) {
            (Scalar::String, _)
                if literal.starts_with('"')
                    || literal.starts_with("r\"")
                    || literal.starts_with("r#") =>
            {
                (!trees.iter().any(|tree| matches!(tree, TokenTree::Punct(_))))
                    .then(|| tokens.clone())
            }
            (Scalar::Int, Some(Number::Int(suffix)))
                if suffix.is_empty() || INT_TYPES.contains(&suffix) =>
            {
                Some(quote!(literal::leaf::<literal::Int, _>(#tokens)))
            }
            (
                Scalar::Float,
                Some(Number::Int("" | "i32" | "u32") | Number::Float("" | "f32" | "f64")),
            ) => Some(quote!(literal::leaf::<literal::Float, _>(#tokens))),
            _ => None,
        }
    }

    /// The trait a Rust expression of type `ty` must implement at `place`,
    /// whose refusal names the GraphQL type.
    fn expect(
        &mut self,
        ty: &TypeRef,
        place: Place,
        site: &Site,
        kind: &TokenStream,
        build: &mut Build,
    ) -> Ident {
        let n = self.next();
        let bound = self.name("Expect", n);
        let krate = self.krate.clone();
        let what = self.what(ty);
        let mut label = accepted(self.schema, ty.nullable());
        let mut impls = Vec::new();
        let mut note = None;
        let inner = ty.nullable();
        if !ty.is_non_null() && place != Place::Field {
            label.push_str(", or an `Option` of it, `None` sending `null`");
            let non_null = TypeRef::NonNull(Box::new(inner.clone()));
            let value = self.expect(&non_null, place, site, kind, build);
            impls.push(quote!(impl<V: #value> #bound for V {}));
            impls.push(quote!(impl<V: #value> #bound for ::core::option::Option<V> {}));
        } else {
            match inner {
                TypeRef::List(item) => {
                    let item = self.expect(
                        item,
                        Place::Item,
                        &Site::Item(Box::new(site.clone())),
                        kind,
                        build,
                    );
                    impls.push(quote! {
                        impl<V: #item> #bound for ::std::vec::Vec<V> {}
                        impl<'a, V: #item> #bound for &'a ::std::vec::Vec<V> {}
                        impl<'a, V: #item> #bound for &'a [V] {}
                        impl<V: #item, const N: usize> #bound for [V; N] {}
                    });
                }
                TypeRef::Named(name) => match self.schema.get(name) {
                    Some(TypeDef::Input { .. }) => {
                        let id = input_object_id(name);
                        impls.push(quote! {
                            impl #bound for #krate::InputObject<#id> {}
                            impl<'a> #bound for &'a #krate::InputObject<#id> {}
                        });
                    }
                    // One impl for the types of the scalar: the refusal is this
                    // trait's, which names the place.
                    _ => impls.push(quote! {
                        #[diagnostic::do_not_recommend]
                        impl<V: literal::Accepts<#kind, Checks>> #bound for V {}
                    }),
                },
                TypeRef::NonNull(_) => unreachable!("`nullable` removes the `!`"),
            }
            if place == Place::Field {
                if self.is_scalar(inner) {
                    note = self.option_note(ty, site);
                } else {
                    impls.push(self.no_option(&bound, ty, site, kind));
                }
            }
        }
        // The type of an `InputObject` is a hash: the message names the
        // expected GraphQL type and leaves the given one to rustc's help.
        let given = if self.holds_input(ty) {
            ""
        } else {
            ", not `{Self}`"
        };
        let message = format!(
            "{} expects {}{given}",
            escape(&site.to_string()),
            escape(&what)
        );
        let label = escape(&label);
        let note = note.map(|note: String| {
            let note = escape(&note);
            quote!(, note = #note)
        });
        // `Validate` as a supertrait: what a value is checked for when it is
        // built follows from its type being accepted.
        build.items.push(quote! {
            #[diagnostic::on_unimplemented(message = #message, label = #label #note)]
            trait #bound: literal::Validate<#kind> {}
            #(#impls)*
        });
        bound
    }

    /// What to write for an `Option` in a nullable field of a scalar, whose
    /// check takes the types of the scalar only: shown with any refusal.
    fn option_note(&self, ty: &TypeRef, site: &Site) -> Option<String> {
        let Site::Field { name, .. } = site else {
            return None;
        };
        (!ty.is_non_null()).then(|| {
            format!(
                "`{name}?: value` takes an `Option`, `None` leaving the field out; `{name}: null` sends `null`"
            )
        })
    }

    fn is_scalar(&self, ty: &TypeRef) -> bool {
        matches!(ty, TypeRef::Named(name) if matches!(self.schema.get(name), Some(TypeDef::Scalar)))
    }

    /// Refuses an `Option` in a field, saying what to write instead: sending
    /// `null` for `None` would clear the field.
    fn no_option(
        &mut self,
        bound: &Ident,
        ty: &TypeRef,
        site: &Site,
        kind: &TokenStream,
    ) -> TokenStream {
        let n = self.next();
        let no_option = self.name("NoOption", n);
        let Site::Field { name, .. } = site else {
            unreachable!("a field has a field site")
        };
        let what = self.what(ty);
        let (message, label) = if ty.is_non_null() {
            (
                format!("{site} is required (`{ty}`): it takes {what}, not an `Option`"),
                "the field is required: give a value".to_string(),
            )
        } else {
            (
                format!(
                    "{site} takes {what}, not an `Option`: write `{name}?: value` to leave the field out when it is `None`, or `{name}: null` to send `null`"
                ),
                format!("`{name}: None` would send `null`, which clears the field"),
            )
        };
        let (message, label) = (escape(&message), escape(&label));
        quote! {
            #[diagnostic::on_unimplemented(message = #message, label = #label)]
            trait #no_option: literal::Validate<#kind> {}
            impl<V: #no_option> #bound for ::core::option::Option<V> {}
        }
    }

    /// A value of the enum `name`, written bare.
    fn enum_value(
        &mut self,
        ident: &Ident,
        name: &str,
        values: &[String],
        deprecated: &BTreeMap<String, String>,
        site: &Site,
    ) -> Result<TokenStream, Error> {
        let given = unraw(ident);
        if !values.contains(&given) {
            let listed: Vec<String> = values
                .iter()
                .take(LISTED_VALUES)
                .map(|value| format!("`{value}`"))
                .collect();
            let mut list = join(&listed, "or");
            if values.len() > LISTED_VALUES {
                list = format!("{}, … ({} values)", listed.join(", "), values.len());
            }
            let run_time = if given.chars().any(char::is_lowercase) {
                format!("; for a string known at run time, write `raw({given})`")
            } else {
                String::new()
            };
            return error(
                ident.span(),
                format!(
                    "`{given}` is not a value of the enum `{name}`{}: {site} takes {list}{run_time}",
                    did_you_mean(&given, values.iter().map(String::as_str))
                ),
            );
        }
        if let Some(reason) = deprecated.get(&given) {
            let note = format!(
                "`{name}.{given}` is deprecated in the Shopify Admin API {}: {reason}",
                self.schema.api_version
            );
            self.items.push(deprecation_warning(ident.span(), &note));
        }
        Ok(quote_spanned!(ident.span()=> #given))
    }

    /// Refuses `raw(…)` where no enum value is expected.
    fn enum_only(&self, ty: &TypeRef, keyword: Span, site: &Site) -> Result<(), Error> {
        if self.enum_values(ty.nullable()).is_some() {
            return Ok(());
        }
        error(
            keyword,
            format!(
                "`raw(…)` is only for a value of an enum: {site} expects {}",
                self.what(ty)
            ),
        )
    }

    fn enum_values(
        &self,
        ty: &TypeRef,
    ) -> Option<(&'a str, &'a [String], &'a BTreeMap<String, String>)> {
        let schema = self.schema;
        match ty {
            TypeRef::Named(name) => match schema.get(name) {
                Some(TypeDef::Enum { values, deprecated }) => {
                    let (name, _) = schema.types.get_key_value(name)?;
                    Some((name.as_str(), values.as_slice(), deprecated))
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn input_fields(&self, ty: &TypeRef) -> Option<(&'a str, &'a BTreeMap<String, InputValue>)> {
        let schema = self.schema;
        match ty {
            TypeRef::Named(name) => match schema.get(name) {
                Some(TypeDef::Input { fields }) => {
                    let (name, _) = schema.types.get_key_value(name)?;
                    Some((name.as_str(), fields))
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// What a Rust expression of type `ty` is checked for when it is built:
    /// the `Int`, `Float` or non-null `JSON` its innermost type is.
    fn kind(&self, ty: &TypeRef) -> TokenStream {
        let (name, non_null) = innermost(ty);
        let kind = match (self.schema.get(name), Scalar::of(name)) {
            (Some(TypeDef::Scalar), Scalar::Int) => quote!(Int),
            (Some(TypeDef::Scalar), Scalar::Float) => quote!(Float),
            (Some(TypeDef::Scalar), Scalar::Boolean) => quote!(Boolean),
            (Some(TypeDef::Scalar), Scalar::Json) if non_null => quote!(Json),
            (Some(TypeDef::Scalar), Scalar::Json) => quote!(MaybeJson),
            (Some(TypeDef::Scalar), _) => quote!(Text),
            _ => quote!(Input),
        };
        quote!(literal::#kind)
    }

    /// Refuses an integer literal outside of the 32 bits of `Int`, where
    /// the innermost type of `ty` is `Int`: written alone, negated, in
    /// `Some(…)`, in parentheses, or as an item of `[…]` or `vec![…]`.
    fn int_literals(&self, tokens: &TokenStream, ty: &TypeRef, site: &Site) -> Result<(), Error> {
        if !matches!(self.schema.get(ty.name()), Some(TypeDef::Scalar))
            || Scalar::of(ty.name()) != Scalar::Int
        {
            return Ok(());
        }
        let mut found = Vec::new();
        int_literals(tokens, &mut found);
        match found
            .into_iter()
            .find(|(value, _)| i32::try_from(*value).is_err())
        {
            Some((_, span)) => error(
                span,
                format!(
                    "The integer is outside the range of `Int`, a 32-bit integer, given for {site}"
                ),
            ),
            None => Ok(()),
        }
    }

    /// The innermost type of `ty` is an input object.
    fn holds_input(&self, ty: &TypeRef) -> bool {
        matches!(self.schema.get(ty.name()), Some(TypeDef::Input { .. }))
    }

    /// The innermost type of `ty` is an enum.
    fn is_enum(&self, ty: &TypeRef) -> bool {
        matches!(self.schema.get(ty.name()), Some(TypeDef::Enum { .. }))
    }

    fn first_value(&self, name: &str) -> &str {
        match self.schema.get(name) {
            Some(TypeDef::Enum { values, .. }) => values.first().map_or("VALUE", String::as_str),
            _ => "VALUE",
        }
    }

    /// A type in words: "a `String`", "a list of `MetafieldsSetInput`, built with …".
    fn what(&self, ty: &TypeRef) -> String {
        match ty.nullable() {
            TypeRef::List(item) => format!("a list of {}", self.items_of(item)),
            other => {
                let name = other.name();
                match self.schema.get(name) {
                    Some(TypeDef::Input { .. }) => {
                        format!("a `{name}`, built with `input!({name} {{ … }})`")
                    }
                    Some(TypeDef::Enum { .. }) => format!("a value of the enum `{name}`"),
                    _ => {
                        let article = if name.starts_with(['A', 'E', 'I', 'O', 'U']) {
                            "an"
                        } else {
                            "a"
                        };
                        match Scalar::of(name) {
                            Scalar::String if !matches!(name, "String") => {
                                format!("{article} `{name}` (a string)")
                            }
                            _ => format!("{article} `{name}`"),
                        }
                    }
                }
            }
        }
    }

    fn items_of(&self, ty: &TypeRef) -> String {
        match ty.nullable() {
            TypeRef::List(item) => format!("lists of {}", self.items_of(item)),
            other => {
                let name = other.name();
                match self.schema.get(name) {
                    Some(TypeDef::Input { .. }) => {
                        format!("`{name}`, built with `input!({name} {{ … }})`")
                    }
                    Some(TypeDef::Enum { .. }) => format!("values of the enum `{name}`"),
                    _ => format!("`{name}`"),
                }
            }
        }
    }

    fn name(&self, what: &str, n: usize) -> Ident {
        format_ident!("__ShopifyApi{what}{n}_{:08x}", self.suffix)
    }

    fn next(&mut self) -> usize {
        self.count += 1;
        self.count
    }
}

/// A hash of the name of the crate being compiled, which cargo gives the
/// macros.
fn crate_suffix() -> u32 {
    let name = std::env::var("CARGO_CRATE_NAME").unwrap_or_default();
    let hash = input_object_id(&name);
    (hash ^ (hash >> 32)) as u32
}

fn literal_after_question(key: &str) -> String {
    format!(
        "`{key}?:` takes a Rust expression, an `Option` or a `shopify_api::Maybe` whose `None` leaves the field out; write `{key}: …` for this value"
    )
}

/// How many values of an enum a message lists.
const LISTED_VALUES: usize = 30;

/// The Rust types accepted for a value of `ty`, the `!` removed.
fn accepted(schema: &Schema, ty: &TypeRef) -> String {
    match ty {
        TypeRef::List(_) => "a `Vec`, a slice or an array".to_string(),
        other => {
            let name = other.name();
            if let Some(TypeDef::Input { .. }) = schema.get(name) {
                return format!("build it with `input!({name} {{ … }})`");
            }
            match Scalar::of(name) {
                Scalar::String => "`String`, `&str`, `&String` or `Cow<str>`",
                Scalar::Int => "any integer type",
                Scalar::Float => "`f64`, `f32`, `i32` or `u32`",
                Scalar::Boolean => "`bool`",
                Scalar::Json => "`serde_json::Value` or `&Value`",
            }
            .to_string()
        }
    }
}

/// The integer literals of `tokens` in the forms
/// [`Literals::int_literals`] reads, and their spans; one beyond `i128` is
/// `i128::MAX`.
fn int_literals(tokens: &TokenStream, found: &mut Vec<(i128, Span)>) {
    let trees: Vec<TokenTree> = tokens.clone().into_iter().collect();
    match trees.as_slice() {
        [TokenTree::Literal(literal)] => {
            if let Some(value) = integer(&literal.to_string()) {
                found.push((value, literal.span()));
            }
        }
        [TokenTree::Punct(minus), TokenTree::Literal(literal)] if minus.as_char() == '-' => {
            if let Some(value) = integer(&literal.to_string()) {
                found.push((value.saturating_neg(), literal.span()));
            }
        }
        [TokenTree::Group(group)] => match group.delimiter() {
            Delimiter::Parenthesis | Delimiter::None => int_literals(&group.stream(), found),
            Delimiter::Bracket => items(&group.stream(), found),
            Delimiter::Brace => {}
        },
        [TokenTree::Ident(some), TokenTree::Group(group)]
            if some == "Some" && group.delimiter() == Delimiter::Parenthesis =>
        {
            int_literals(&group.stream(), found);
        }
        [
            TokenTree::Ident(vec),
            TokenTree::Punct(bang),
            TokenTree::Group(group),
        ] if vec == "vec" && bang.as_char() == '!' && group.delimiter() == Delimiter::Bracket => {
            items(&group.stream(), found);
        }
        _ => {}
    }

    fn items(tokens: &TokenStream, found: &mut Vec<(i128, Span)>) {
        let mut item = TokenStream::new();
        for tree in tokens.clone() {
            match &tree {
                TokenTree::Punct(comma) if comma.as_char() == ',' => {
                    int_literals(&std::mem::take(&mut item), found);
                }
                _ => item.extend([tree]),
            }
        }
        int_literals(&item, found);
    }
}

/// The value of the text of an integer literal: `2_147_483_648_i64`,
/// `0x8000_0000`, `0b1`; `None` for any other literal.
fn integer(text: &str) -> Option<i128> {
    const SUFFIXES: [&str; 12] = [
        "i8", "i16", "i32", "i64", "i128", "isize", "u8", "u16", "u32", "u64", "u128", "usize",
    ];
    let (radix, digits) = [(16, "0x"), (8, "0o"), (2, "0b")]
        .into_iter()
        .find_map(|(radix, prefix)| Some((radix, text.strip_prefix(prefix)?)))
        .unwrap_or((10, text));
    let digits = SUFFIXES
        .iter()
        .find_map(|suffix| digits.strip_suffix(suffix))
        .unwrap_or(digits);
    let digits: String = digits.chars().filter(|&digit| digit != '_').collect();
    if digits.is_empty() || !digits.chars().all(|digit| digit.is_digit(radix)) {
        return None;
    }
    Some(
        u128::from_str_radix(&digits, radix).map_or(i128::MAX, |value| {
            i128::try_from(value).unwrap_or(i128::MAX)
        }),
    )
}

/// The innermost named type of `ty`, and whether it is non-null.
fn innermost(ty: &TypeRef) -> (&str, bool) {
    match ty {
        TypeRef::Named(name) => (name, false),
        TypeRef::NonNull(inner) => match &**inner {
            TypeRef::Named(name) => (name, true),
            other => innermost(other),
        },
        TypeRef::List(item) => innermost(item),
    }
}

/// The Rust expressions of a value, which are the arguments of the method
/// that builds it, and the traits that check them.
#[derive(Default)]
struct Build {
    items: Vec<TokenStream>,
    generics: Vec<TokenStream>,
    params: Vec<TokenStream>,
    args: Vec<TokenStream>,
}

impl Build {
    /// Takes `arg` as an argument of type `ty`: the name the code that
    /// builds the value refers to it by.
    fn argument(
        &mut self,
        generic: Option<TokenStream>,
        ty: TokenStream,
        arg: TokenStream,
    ) -> TokenStream {
        let name = format_ident!("value{}", self.params.len());
        self.generics.extend(generic);
        self.params.push(quote!(#name: #ty));
        self.args.push(arg);
        quote!(#name)
    }

    /// The code of the JSON of the value, or of why it is refused: a
    /// `Result<Box<RawValue>, Invalid>`, a type that does not depend on the
    /// arguments, so that a refused argument is reported once, at it.
    fn finish(self, body: TokenStream, krate: &TokenStream) -> TokenStream {
        let Self {
            items,
            generics,
            params,
            args,
        } = self;
        let json = quote!(literal::to_raw(&#body));
        // `literal`, in a block that no expression of the caller's is in.
        let path = quote!(#[allow(unused_imports)] use #krate::__private::literal;);
        if args.is_empty() {
            return quote!({ #path #json });
        }
        // The parameters shadow functions of these names, and so no
        // constant: `literal::names` has the first ones.
        let names = (NAMES..params.len()).map(|n| {
            let name = format_ident!("value{n}");
            quote!(#[allow(dead_code)] fn #name() {})
        });
        quote! {
            ({
                #path
                #[allow(unused_imports)]
                use #krate::__private::literal::names::*;
                #(#names)*
                #(#items)*
                struct Checks;
                impl Checks {
                    #[allow(clippy::too_many_arguments)]
                    fn build<#(#generics),*>(self, #(#params),*) -> ::core::result::Result<
                        ::std::boxed::Box<#krate::__private::serde_json::value::RawValue>,
                        literal::Invalid,
                    > {
                        #json
                    }
                }
                Checks
            })
            .build(#(#args),*)
        }
    }
}

/// How many names of parameters `literal::names` has.
const NAMES: usize = 64;

const INT_TYPES: [&str; 10] = [
    "i8", "i16", "i32", "i64", "isize", "u8", "u16", "u32", "u64", "usize",
];

/// A number literal and its suffix.
enum Number<'a> {
    Int(&'a str),
    Float(&'a str),
}

/// The kind of the number literal `text`, if it is one.
fn number(text: &str) -> Option<Number<'_>> {
    if !text.starts_with(|first: char| first.is_ascii_digit()) {
        return None;
    }
    if integer(text).is_some() {
        let suffix = INT_TYPES
            .iter()
            .chain(&["i128", "u128"])
            .find(|suffix| text.ends_with(*suffix))
            .copied()
            .unwrap_or("");
        return Some(Number::Int(suffix));
    }
    let (digits, suffix) = ["f32", "f64"]
        .iter()
        .find_map(|suffix| Some((text.strip_suffix(suffix)?, *suffix)))
        .unwrap_or((text, ""));
    digits
        .chars()
        .all(|character| {
            character.is_ascii_digit() || matches!(character, '.' | '_' | 'e' | 'E' | '+' | '-')
        })
        .then_some(Number::Float(suffix))
}

/// An expression of the caller's as an argument, in parentheses of the
/// macro's at its place: its braces are not a method argument of the
/// caller's to lint, and a refusal points at it.
pub fn argument(expr: &Expr) -> TokenStream {
    let mut group = Group::new(Delimiter::Parenthesis, expr.tokens.clone());
    group.set_span(Span::mixed_site().located_at(expr.span));
    TokenTree::Group(group).into()
}

fn ident_expr(ident: &Ident) -> Expr {
    Expr {
        tokens: quote!(#ident),
        span: ident.span(),
    }
}

/// The name of an identifier, without the `r#` of a raw one.
fn unraw(ident: &Ident) -> String {
    let name = ident.to_string();
    match name.strip_prefix("r#") {
        Some(name) => name.to_string(),
        None => name,
    }
}

/// `a`, `a and b`, `a, b and c`.
fn join(items: &[String], last: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [init @ .., tail] => format!("{} {last} {tail}", init.join(", ")),
    }
}

/// Text for `#[diagnostic::on_unimplemented]`, where braces are placeholders.
fn escape(text: &str) -> String {
    text.replace('{', "{{").replace('}', "}}")
}
