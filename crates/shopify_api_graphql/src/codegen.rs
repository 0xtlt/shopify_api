//! Rust types for the result and the variables of an analyzed operation.

use std::collections::{BTreeMap, BTreeSet};

use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

use crate::analyze::{
    FieldBase, Operation, Scalar, SelectedField, Selection, Variable, VariableBase,
};
use crate::schema::{InputValue, Schema, TypeDef, TypeRef};

/// The name of the type generated for the `data` of an operation.
pub const DATA_TYPE: &str = "Data";
/// The name of the type generated for the variables of an operation.
pub const VARIABLES_TYPE: &str = "Variables";

/// Identifiers that cannot be written `r#name`.
const UNRAWABLE: [&str; 5] = ["self", "Self", "super", "crate", "_"];
const KEYWORDS: &[&str] = &[
    "abstract", "as", "async", "await", "become", "box", "break", "const", "continue", "do", "dyn",
    "else", "enum", "extern", "false", "final", "fn", "for", "gen", "if", "impl", "in", "let",
    "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref", "return",
    "static", "struct", "trait", "true", "try", "type", "typeof", "unsafe", "unsized", "use",
    "virtual", "where", "while", "yield",
];

/// `lineItems` → `line_items`, `__typename` → `typename`, `HTMLBody` → `html_body`.
fn snake_case(name: &str) -> String {
    let characters: Vec<char> = name.trim_matches('_').chars().collect();
    let mut snake = String::new();
    for (index, character) in characters.iter().enumerate() {
        if character.is_uppercase() {
            let after_lower = index > 0
                && (characters[index - 1].is_lowercase() || characters[index - 1].is_ascii_digit());
            let before_lower = index > 0
                && characters[index - 1].is_uppercase()
                && characters
                    .get(index + 1)
                    .is_some_and(|next| next.is_lowercase());
            if (after_lower || before_lower) && !snake.ends_with('_') {
                snake.push('_');
            }
            snake.extend(character.to_lowercase());
        } else {
            snake.push(*character);
        }
    }
    if snake.is_empty() || snake.starts_with(|first: char| first.is_ascii_digit()) {
        snake.insert(0, '_');
    }
    snake
}

/// `lineItems` → `LineItems`, `line_items` → `LineItems`, `__typename` → `Typename`.
fn pascal_case(name: &str) -> String {
    let mut pascal = String::new();
    let mut capitalize = true;
    for character in name.chars() {
        if character == '_' {
            capitalize = true;
        } else if capitalize {
            pascal.extend(character.to_uppercase());
            capitalize = false;
        } else {
            pascal.push(character);
        }
    }
    pascal
}

/// The Rust field for a GraphQL name: snake case, raw when it is a keyword,
/// with the `serde` attribute that restores the name when it differs.
fn field_ident(name: &str, taken: &mut BTreeSet<String>) -> (Ident, Option<TokenStream>) {
    let mut snake = snake_case(name);
    if UNRAWABLE.contains(&snake.as_str()) {
        snake.push('_');
    }
    while !taken.insert(snake.clone()) {
        snake.push('_');
    }
    let rename = (snake != name).then(|| quote!(#[serde(rename = #name)]));
    let ident = if KEYWORDS.contains(&snake.as_str()) {
        Ident::new_raw(&snake, Span::call_site())
    } else {
        Ident::new(&snake, Span::call_site())
    };
    (ident, rename)
}

fn scalar_type(scalar: Scalar, krate: &TokenStream) -> TokenStream {
    match scalar {
        Scalar::String => quote!(::std::string::String),
        Scalar::Int => quote!(::core::primitive::i64),
        Scalar::Float => quote!(::core::primitive::f64),
        Scalar::Boolean => quote!(::core::primitive::bool),
        Scalar::Json => quote!(#krate::__private::serde_json::Value),
    }
}

/// Wraps `base` in the `Option`s and `Vec`s of a GraphQL type.
fn wrap(ty: &TypeRef, base: &TokenStream) -> TokenStream {
    match ty {
        TypeRef::NonNull(inner) => wrap_non_null(inner, base),
        nullable => {
            let inner = wrap_non_null(nullable, base);
            quote!(::core::option::Option<#inner>)
        }
    }
}

fn wrap_non_null(ty: &TypeRef, base: &TokenStream) -> TokenStream {
    match ty {
        TypeRef::Named(_) => base.clone(),
        TypeRef::NonNull(inner) => wrap_non_null(inner, base),
        TypeRef::List(item) => {
            let item = wrap(item, base);
            quote!(::std::vec::Vec<#item>)
        }
    }
}

/// The path of `serde` in the generated code, as `#[serde(crate = "…")]` takes it.
fn serde_path(krate: &TokenStream) -> String {
    format!("{krate}::__private::serde").replace(' ', "")
}

// ─── Result ──────────────────────────────────────────────────────────

struct DataTypes<'a> {
    krate: &'a TokenStream,
    names: BTreeSet<String>,
    items: Vec<TokenStream>,
}

impl DataTypes<'_> {
    fn object(&mut self, selection: &Selection, name: String) -> Ident {
        let mut name = name;
        while !self.names.insert(name.clone()) {
            name.push('_');
        }
        let ident = Ident::new(&name, Span::call_site());

        let mut taken = BTreeSet::new();
        let fields: Vec<TokenStream> = selection
            .fields
            .iter()
            .map(|field| self.field(field, &name, &mut taken))
            .collect();

        let krate = self.krate;
        let serde = serde_path(krate);
        let on = selection.types.join("` or `");
        let doc = if name == DATA_TYPE {
            format!(" The `data` of the operation, selected on `{on}`.")
        } else {
            format!(" Selection on `{on}`.")
        };
        self.items.push(quote! {
            #[doc = #doc]
            #[derive(::core::fmt::Debug, ::core::clone::Clone, ::core::cmp::PartialEq, #krate::__private::serde::Serialize, #krate::__private::serde::Deserialize)]
            #[serde(crate = #serde)]
            #[allow(dead_code)]
            pub struct #ident {
                #(#fields)*
            }
        });
        ident
    }

    fn field(
        &mut self,
        field: &SelectedField,
        parent: &str,
        taken: &mut BTreeSet<String>,
    ) -> TokenStream {
        let (ident, rename) = field_ident(&field.key, taken);
        let base = match &field.base {
            FieldBase::Scalar(scalar) => scalar_type(*scalar, self.krate),
            FieldBase::Object(selection) => {
                let nested = self.object(selection, format!("{parent}{}", pascal_case(&field.key)));
                quote!(#nested)
            }
        };
        let optional = field.conditional || !field.ty.is_non_null();
        let ty = if field.conditional && field.ty.is_non_null() {
            let inner = wrap(&field.ty, &base);
            quote!(::core::option::Option<#inner>)
        } else {
            wrap(&field.ty, &base)
        };

        let default = optional.then(|| quote!(#[serde(default)]));
        let doc = format!(" `{}: {}`", field.key, field.ty);
        quote! {
            #[doc = #doc]
            #rename
            #default
            pub #ident: #ty,
        }
    }
}

/// The public structs describing the `data` of `operation`: [`DATA_TYPE`] and
/// one struct per nested selection, named after its path (`DataOrdersNodes`).
///
/// `krate` is the path of the `shopify_api` crate in the generated code.
pub fn data_types(operation: &Operation, krate: &TokenStream) -> TokenStream {
    let mut types = DataTypes {
        krate,
        names: BTreeSet::new(),
        items: Vec::new(),
    };
    types.object(&operation.data, DATA_TYPE.to_string());
    let items = types.items;
    quote!(#(#items)*)
}

// ─── Variables ───────────────────────────────────────────────────────

/// The Rust type a variable value is converted to, or `None` when the variable
/// holds an input object, which has no type outside a `graphql!` module.
pub fn variable_type(variable: &Variable, krate: &TokenStream) -> Option<TokenStream> {
    let base = match variable.base {
        VariableBase::Scalar(scalar) => scalar_type(scalar, krate),
        VariableBase::Enum => scalar_type(Scalar::String, krate),
        VariableBase::Input => return None,
    };
    Some(wrap(&variable.ty, &base))
}

struct InputTypes<'a> {
    schema: &'a Schema,
    krate: &'a TokenStream,
    generated: BTreeSet<String>,
    items: Vec<TokenStream>,
}

impl InputTypes<'_> {
    /// The Rust type of a named input type, generating input objects on the way.
    fn named(&mut self, name: &str) -> TokenStream {
        match self.schema.get(name) {
            Some(TypeDef::Input { fields }) => {
                if self.generated.insert(name.to_string()) {
                    self.input_object(name, fields);
                }
                let ident = Ident::new(name, Span::call_site());
                quote!(#ident)
            }
            Some(TypeDef::Enum { .. }) => scalar_type(Scalar::String, self.krate),
            _ => scalar_type(Scalar::of(name), self.krate),
        }
    }

    fn input_object(&mut self, name: &str, fields: &BTreeMap<String, InputValue>) {
        let mut taken = BTreeSet::new();
        let fields: Vec<TokenStream> = fields
            .iter()
            .map(|(field, value)| {
                let mut base = self.named(value.ty.name());
                // A value cannot contain itself: break direct cycles with a box.
                let nested_directly = !contains_list(&value.ty);
                if nested_directly && self.reaches(value.ty.name(), name) {
                    base = quote!(::std::boxed::Box<#base>);
                }
                let note = value.deprecation.as_ref().map(|reason| {
                    format!(
                        "`{name}.{field}` is deprecated in the Shopify Admin API {}: {reason}",
                        self.schema.api_version
                    )
                });
                input_field(field, value, note, &base, &mut taken)
            })
            .collect();

        let doc = format!(" The input object `{name}`. Fields left to `None` are not sent.");
        self.items
            .push(input_struct(name, &doc, &fields, self.krate));
    }

    /// Whether a value of input type `from` can hold one of type `to` without a list in between.
    fn reaches(&self, from: &str, to: &str) -> bool {
        let mut seen = BTreeSet::new();
        let mut stack = vec![from];
        while let Some(name) = stack.pop() {
            if name == to {
                return true;
            }
            if !seen.insert(name) {
                continue;
            }
            if let Some(TypeDef::Input { fields }) = self.schema.get(name) {
                stack.extend(
                    fields
                        .values()
                        .filter(|value| !contains_list(&value.ty))
                        .map(|value| value.ty.name()),
                );
            }
        }
        false
    }
}

fn contains_list(ty: &TypeRef) -> bool {
    match ty {
        TypeRef::Named(_) => false,
        TypeRef::List(_) => true,
        TypeRef::NonNull(inner) => contains_list(inner),
    }
}

/// A field of an input object or of the variables. A value that may be left
/// out is an `Option` that is not sent when `None`. A deprecated input field
/// is marked `#[deprecated]` with `note`, so that setting it warns.
fn input_field(
    name: &str,
    value: &InputValue,
    note: Option<String>,
    base: &TokenStream,
    taken: &mut BTreeSet<String>,
) -> TokenStream {
    let (ident, rename) = field_ident(name, taken);
    let ty = &value.ty;
    let doc = format!(" `{name}: {ty}`");
    let deprecated = note.map(|note| quote!(#[deprecated(note = #note)]));
    if value.is_required() {
        let ty = wrap(ty, base);
        quote! {
            #[doc = #doc]
            #deprecated
            #rename
            pub #ident: #ty,
        }
    } else {
        // `wrap` of a nullable type is already an `Option`.
        let ty = wrap(ty.nullable(), base);
        quote! {
            #[doc = #doc]
            #deprecated
            #rename
            #[serde(skip_serializing_if = "::core::option::Option::is_none")]
            pub #ident: #ty,
        }
    }
}

/// A struct that is only sent: an input object, or the variables.
fn input_struct(name: &str, doc: &str, fields: &[TokenStream], krate: &TokenStream) -> TokenStream {
    let ident = Ident::new(name, Span::call_site());
    let serde = serde_path(krate);
    quote! {
        #[doc = #doc]
        #[derive(::core::fmt::Debug, ::core::clone::Clone, ::core::cmp::PartialEq, ::core::default::Default, #krate::__private::serde::Serialize)]
        #[serde(crate = #serde)]
        pub struct #ident {
            #(#fields)*
        }
    }
}

/// [`VARIABLES_TYPE`] and the input objects it refers to, named as in the schema.
pub fn variables_types(schema: &Schema, operation: &Operation, krate: &TokenStream) -> TokenStream {
    let mut inputs = InputTypes {
        schema,
        krate,
        generated: BTreeSet::new(),
        items: Vec::new(),
    };
    let mut taken = BTreeSet::new();
    let fields: Vec<TokenStream> = operation
        .variables
        .iter()
        .map(|variable| {
            let base = inputs.named(variable.ty.name());
            let value = InputValue {
                ty: variable.ty.clone(),
                // A variable with a default may be left out, as an argument with one.
                has_default: variable.has_default,
                deprecation: None,
            };
            input_field(&variable.name, &value, None, &base, &mut taken)
        })
        .collect();

    let items = inputs.items;
    let variables = input_struct(
        VARIABLES_TYPE,
        " The variables of the operation. Those left to `None` are not sent.",
        &fields,
        krate,
    );
    quote! {
        #(#items)*

        #variables
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_names() {
        for (name, snake, pascal) in [
            ("id", "id", "Id"),
            ("lineItems", "line_items", "LineItems"),
            ("totalPriceSet", "total_price_set", "TotalPriceSet"),
            ("__typename", "typename", "Typename"),
            ("HTMLBody", "html_body", "HTMLBody"),
            ("sku2", "sku2", "Sku2"),
            ("address1", "address1", "Address1"),
            ("already_snake", "already_snake", "AlreadySnake"),
            ("iOS", "i_os", "IOS"),
            ("2fa", "_2fa", "2fa"),
        ] {
            assert_eq!(snake_case(name), snake, "{name}");
            assert_eq!(pascal_case(name), pascal, "{name}");
        }
    }

    #[test]
    fn field_identifiers_avoid_keywords_and_duplicates() {
        let mut taken = BTreeSet::new();
        let mut field = |name: &str| {
            let (ident, rename) = field_ident(name, &mut taken);
            (
                ident.to_string(),
                rename.map(|attribute| attribute.to_string()),
            )
        };
        // A raw identifier still has the name of the field: nothing to restore.
        assert_eq!(field("type"), ("r#type".to_string(), None));
        assert_eq!(field("id"), ("id".to_string(), None));
        let renamed = |key: &str| Some(format!("# [serde (rename = \"{key}\")]"));
        assert_eq!(field("self"), ("self_".to_string(), renamed("self")));
        assert_eq!(
            field("lineItems"),
            ("line_items".to_string(), renamed("lineItems"))
        );
        assert_eq!(
            field("line_items"),
            ("line_items_".to_string(), renamed("line_items"))
        );
    }
}
