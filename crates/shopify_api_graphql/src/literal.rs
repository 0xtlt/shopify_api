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

use proc_macro2::{Ident, Span, TokenStream};
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

/// Checks values against a schema and writes their code. The code refers
/// to items [`Literals::finish`] returns, to put in the same block.
pub struct Literals<'a> {
    schema: &'a Schema,
    krate: TokenStream,
    items: Vec<TokenStream>,
    count: usize,
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
        }
    }

    /// The traits, functions and warnings the code written so far needs.
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
        let converted = |value: Expr| VariableValue::Converted {
            ty: variable_type(variable, &self.krate)
                .expect("a variable of a scalar or an enum has a Rust type"),
            value,
        };
        match value {
            Value::Rust(expr) if scalar => return Ok(converted(expr.clone())),
            Value::Ident(ident) if scalar => return Ok(converted(ident_expr(ident))),
            Value::List(List {
                items,
                rust: Some(rust),
                ..
            }) if scalar
                && items
                    .iter()
                    .all(|item| matches!(item, Value::Rust(_) | Value::Ident(_))) =>
            {
                return Ok(converted(rust.clone()));
            }
            Value::Raw { keyword, value } => {
                self.enum_only(ty, *keyword, &site)?;
                return Ok(converted(value.clone()));
            }
            _ => {}
        }
        let code = self.value(value, ty, &site, Place::Variable)?;
        let krate = &self.krate;
        Ok(VariableValue::Literal(
            quote!(#krate::__private::literal::to_raw(&#code)),
        ))
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
        let code = self.object(name, fields, object)?;
        let krate = &self.krate;
        let id = input_object_id(name);
        Ok(quote_spanned! {span=>
            #krate::InputObject::<#id>::__new(#name, &#code)
        })
    }

    /// The code of a value of type `ty`: an expression of a serializable value.
    fn value(
        &mut self,
        value: &Value,
        ty: &TypeRef,
        site: &Site,
        place: Place,
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
                let tokens = &value.tokens;
                Ok(quote_spanned!(value.span=> #krate::__private::literal::raw(#tokens)))
            }
            Value::Ident(ident) => match self.enum_values(inner) {
                Some((name, values, deprecated)) => {
                    self.enum_value(ident, name, values, deprecated, site)
                }
                None => self.expression(&ident_expr(ident), ty, site, place),
            },
            Value::Object(object) => match self.input_fields(inner) {
                Some((name, fields)) => self.object(name, fields, object),
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
                        items.push(self.value(value, item, &site, Place::Item)?);
                    }
                    let nested = items
                        .iter()
                        .rev()
                        .fold(quote!(()), |rest, item| quote!((#item, #rest)));
                    Ok(quote!(#krate::__private::literal::List(#nested)))
                }
                (_, Some(rust)) => self.expression(rust, ty, site, place),
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
                let value = self.value(&comprehension.item, item, &site, Place::Item)?;
                let For {
                    pattern,
                    iterable,
                    filter,
                    ..
                } = &**comprehension;
                // Each item is written in its turn of the loop, so that it may
                // borrow what the loop gives.
                let items = Ident::new("__shopify_api_items", Span::mixed_site());
                let push = quote!(#items.push(#krate::__private::literal::to_raw(&#value)););
                let body = match filter {
                    Some(filter) => quote!(if #filter { #push }),
                    None => push,
                };
                Ok(quote! {{
                    let mut #items = ::std::vec::Vec::new();
                    for #pattern in #iterable { #body }
                    #items
                }})
            }
            Value::Rust(expr) => self.expression(expr, ty, site, place),
        }
    }

    /// The fields of an object literal, for the input object `name`.
    fn object(
        &mut self,
        name: &str,
        definitions: &BTreeMap<String, InputValue>,
        object: &Object,
    ) -> Result<TokenStream, Error> {
        let krate = self.krate.clone();
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
                    let value = self.optional(field, definition, &site)?;
                    quote!(#krate::__private::literal::Optional(#key, #value))
                }
                None => {
                    let value = self.value(&field.value, &definition.ty, &site, Place::Field)?;
                    quote!(#krate::__private::literal::Field(#key, #value))
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
        Ok(quote!(#krate::__private::literal::Object(#nested)))
    }

    /// `field?: value`, the field being optional: `Maybe<V>` of the value.
    fn optional(
        &mut self,
        field: &Field,
        definition: &InputValue,
        site: &Site,
    ) -> Result<TokenStream, Error> {
        let krate = self.krate.clone();
        let key = &field.name;
        // Non-null with a default: absent is fine, `null` is not.
        let null_refused = definition.ty.is_non_null();
        let expr = match &field.value {
            Value::Raw { keyword, value } => {
                self.enum_only(&definition.ty, *keyword, site)?;
                let tokens = &value.tokens;
                let convert = if null_refused {
                    quote!(raw_option)
                } else {
                    quote!(raw_optional)
                };
                return Ok(
                    quote_spanned!(value.span=> #krate::__private::literal::#convert(#tokens)),
                );
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

        let value_bound = self.expect(&definition.ty, Place::Field, site);
        let n = self.next();
        let bound = format_ident!("__ShopifyApiOptional{n}");
        let check = format_ident!("__shopify_api_optional{n}");
        let what = escape(&self.what(&definition.ty));
        let (message, label, maybe_bound) = if null_refused {
            let no_null = format_ident!("__ShopifyApiNoNull{n}");
            let refusal = escape(&format!(
                "{site} is non-null (`{}`): `{key}?:` takes an `Option`, not a `Maybe`, whose `Null` it refuses",
                definition.ty
            ));
            self.items.push(quote! {
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
        // `IntoMaybe` as a supertrait: one refusal, which names the field.
        self.items.push(quote! {
            #[diagnostic::on_unimplemented(message = #message, label = #label)]
            trait #bound<T>: #krate::__private::literal::IntoMaybe<T> {}
            impl<V: #value_bound> #bound<V> for ::core::option::Option<V> {}
            impl<V: #maybe_bound> #bound<V> for #krate::Maybe<V> {}
            fn #check<T, V: #bound<T>>(value: V) -> #krate::Maybe<T> {
                #krate::__private::literal::IntoMaybe::into_maybe(value)
            }
        });
        let tokens = &expr.tokens;
        Ok(quote_spanned!(expr.span=> #check(#tokens)))
    }

    /// A Rust expression for a value of type `ty`, checked by a trait.
    fn expression(
        &mut self,
        expr: &Expr,
        ty: &TypeRef,
        site: &Site,
        place: Place,
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
        let bound = self.expect(ty, place, site);
        let check = format_ident!("__shopify_api_check{}", self.next());
        self.items.push(quote! {
            fn #check<V: #bound>(value: V) -> V {
                value
            }
        });
        let tokens = &expr.tokens;
        Ok(quote_spanned!(expr.span=> #check(#tokens)))
    }

    /// The trait a Rust expression of type `ty` must implement at `place`,
    /// whose refusal names the GraphQL type.
    fn expect(&mut self, ty: &TypeRef, place: Place, site: &Site) -> Ident {
        let bound = format_ident!("__ShopifyApiExpect{}", self.next());
        let krate = self.krate.clone();
        let what = self.what(ty);
        let mut label = accepted(self.schema, ty.nullable());
        let mut impls = Vec::new();
        let inner = ty.nullable();
        if !ty.is_non_null() && place != Place::Field {
            label.push_str(", or an `Option` of it, `None` sending `null`");
            let non_null = TypeRef::NonNull(Box::new(inner.clone()));
            let value = self.expect(&non_null, place, site);
            impls.push(quote!(impl<V: #value> #bound for V {}));
            impls.push(quote!(impl<V: #value> #bound for ::core::option::Option<V> {}));
        } else {
            match inner {
                TypeRef::List(item) => {
                    let item = self.expect(item, Place::Item, &Site::Item(Box::new(site.clone())));
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
                    _ => {
                        for ty in scalar_types(Scalar::of(name), &krate) {
                            impls.push(quote!(impl<'a> #bound for #ty {}));
                        }
                    }
                },
                TypeRef::NonNull(_) => unreachable!("`nullable` removes the `!`"),
            }
            if place == Place::Field {
                impls.push(self.no_option(&bound, ty, site));
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
        self.items.push(quote! {
            #[diagnostic::on_unimplemented(message = #message, label = #label)]
            trait #bound {}
            #(#impls)*
        });
        bound
    }

    /// Refuses an `Option` in a field, saying what to write instead: sending
    /// `null` for `None` would clear the field.
    fn no_option(&mut self, bound: &Ident, ty: &TypeRef, site: &Site) -> TokenStream {
        let n = self.next();
        let no_option = format_ident!("__ShopifyApiNoOption{n}");
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
            trait #no_option {}
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

    fn next(&mut self) -> usize {
        self.count += 1;
        self.count
    }
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

/// The Rust types of a scalar, as `IntoVariable` accepts them; they may
/// borrow for `'a`.
fn scalar_types(scalar: Scalar, krate: &TokenStream) -> Vec<TokenStream> {
    match scalar {
        Scalar::String => vec![
            quote!(::std::string::String),
            quote!(&'a ::core::primitive::str),
            quote!(&'a ::std::string::String),
            quote!(::std::borrow::Cow<'a, ::core::primitive::str>),
        ],
        Scalar::Int => [
            "i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "usize", "isize",
        ]
        .iter()
        .map(|name| {
            let ident = Ident::new(name, Span::call_site());
            quote!(::core::primitive::#ident)
        })
        .collect(),
        Scalar::Float => ["f32", "f64", "i32", "u32"]
            .iter()
            .map(|name| {
                let ident = Ident::new(name, Span::call_site());
                quote!(::core::primitive::#ident)
            })
            .collect(),
        Scalar::Boolean => vec![quote!(::core::primitive::bool)],
        Scalar::Json => vec![
            quote!(#krate::__private::serde_json::Value),
            quote!(&'a #krate::__private::serde_json::Value),
        ],
    }
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
