//! Rust types for the result and the variables of an analyzed operation.

use std::collections::BTreeSet;

use proc_macro2::{Ident, Span, TokenStream};
use quote::{quote, quote_spanned};

use crate::analyze::{
    FieldBase, Operation, Scalar, SelectedField, Selection, Variable, VariableBase,
};
use crate::schema::{Schema, TypeDef, TypeRef};

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
        } else if name == crate::bulk::ITEM_TYPE {
            format!(
                " A top-level object of the bulk query, with the objects of its nested connections, selected on `{on}`."
            )
        } else {
            format!(" Selection on `{on}`.")
        };
        self.items.push(quote! {
            #[doc = #doc]
            #[derive(::core::fmt::Debug, ::core::clone::Clone, ::core::cmp::PartialEq, #krate::__private::serde::Serialize, #krate::__private::serde::Deserialize)]
            #[serde(crate = #serde)]
            #[allow(dead_code, reason = "the caller may read only some fields")]
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

        let default = (optional || field.connection).then(|| quote!(#[serde(default)]));
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

/// The structs of the items of a bulk query: `name`, and one struct per
/// nested selection, named after its path (`ItemVariants`).
pub fn item_types(selection: &Selection, name: &str, krate: &TokenStream) -> TokenStream {
    let mut types = DataTypes {
        krate,
        names: BTreeSet::new(),
        items: Vec::new(),
    };
    types.object(selection, name.to_string());
    let items = types.items;
    quote!(#(#items)*)
}

/// The Rust fields of the struct of `selection`, by response key, as
/// [`item_types`] and [`data_types`] name them.
pub(crate) fn field_idents(selection: &Selection) -> Vec<(String, Ident)> {
    let mut taken = BTreeSet::new();
    selection
        .fields
        .iter()
        .map(|field| (field.key.clone(), field_ident(&field.key, &mut taken).0))
        .collect()
}

// ─── Variables ───────────────────────────────────────────────────────

/// The Rust type a variable value is converted to, or `None` when the variable
/// holds an input object, which is an [`InputObject`](https://docs.rs/shopify_api/latest/shopify_api/struct.InputObject.html).
pub fn variable_type(variable: &Variable, krate: &TokenStream) -> Option<TokenStream> {
    let base = match variable.base {
        VariableBase::Scalar(scalar) => scalar_type(scalar, krate),
        VariableBase::Enum => scalar_type(Scalar::String, krate),
        VariableBase::Input => return None,
    };
    Some(wrap(&variable.ty, &base))
}

/// The `TYPE` of `shopify_api::InputObject<TYPE>` for the input object
/// `name`: the 64-bit FNV-1a hash of the name, the same in every crate.
pub fn input_object_id(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A compiler warning at `span` that gives `note`: the use of a
/// `#[deprecated]` constant.
pub fn deprecation_warning(span: Span, note: &str) -> TokenStream {
    quote_spanned! {span=>
        const _: () = {
            #[deprecated(note = #note)]
            const SHOPIFY_DEPRECATION: () = ();
            SHOPIFY_DEPRECATION
        };
    }
}

/// The input objects a value of input type `name` may hold, `name` included.
fn reachable_inputs<'a>(schema: &'a Schema, name: &'a str, found: &mut BTreeSet<&'a str>) {
    if let Some(TypeDef::Input { fields, .. }) = schema.get(name)
        && found.insert(name)
    {
        for field in fields.values() {
            reachable_inputs(schema, field.ty.name(), found);
        }
    }
}

/// [`VARIABLES_TYPE`], `operation(variables)`, `From<Variables>` for the
/// operation, and a type alias, named as in the schema, for each input object
/// the variables may hold.
///
/// `constructor` builds the operation without its variables; `krate` is the
/// path of the `shopify_api` crate in the generated code.
pub fn variables_types(
    schema: &Schema,
    operation: &Operation,
    krate: &TokenStream,
    constructor: &TokenStream,
) -> TokenStream {
    let data = Ident::new(DATA_TYPE, Span::call_site());
    let target = quote!(#krate::__private::Operation<#data>);
    variables_types_for(
        schema,
        operation,
        krate,
        constructor,
        &target,
        "`Shopify::execute`",
    )
}

/// [`variables_types`] for an operation of type `target`, which `runner`
/// runs: a bulk query or a bulk mutation.
pub fn variables_types_for(
    schema: &Schema,
    operation: &Operation,
    krate: &TokenStream,
    constructor: &TokenStream,
    target: &TokenStream,
    runner: &str,
) -> TokenStream {
    let mut inputs = BTreeSet::new();
    for variable in &operation.variables {
        reachable_inputs(schema, variable.ty.name(), &mut inputs);
    }
    let aliases = inputs.iter().map(|name| {
        let ident = Ident::new(name, Span::call_site());
        let id = input_object_id(name);
        let doc = format!(" The input object `{name}`: build it with `input!({name} {{ … }})`.");
        quote! {
            #[doc = #doc]
            #[allow(dead_code, reason = "the caller may not name every input object")]
            pub type #ident = #krate::__private::InputObject<#id>;
        }
    });

    let mut taken = BTreeSet::new();
    let mut fields = Vec::new();
    let mut setters = Vec::new();
    for variable in &operation.variables {
        let base = match variable.base {
            VariableBase::Scalar(scalar) => scalar_type(scalar, krate),
            VariableBase::Enum => scalar_type(Scalar::String, krate),
            VariableBase::Input => {
                let ident = Ident::new(variable.ty.name(), Span::call_site());
                quote!(#ident)
            }
        };
        let (ident, rename) = field_ident(&variable.name, &mut taken);
        let key = &variable.name;
        let doc = format!(" `${key}: {}`", variable.ty);
        if variable.is_required() {
            let ty = wrap(&variable.ty, &base);
            fields.push(quote! {
                #[doc = #doc]
                #rename
                pub #ident: #ty,
            });
            setters.push(quote! {
                let operation = operation.__variable(#key, variables.#ident);
            });
        } else {
            // `wrap` of a nullable type is already an `Option`.
            let ty = wrap(variable.ty.nullable(), &base);
            fields.push(quote! {
                #[doc = #doc]
                #rename
                #[serde(skip_serializing_if = "::core::option::Option::is_none")]
                pub #ident: #ty,
            });
            // `None` leaves the variable out. A non-null variable with a
            // default takes its value out of the `Some`, checked as non-null.
            let value = if variable.ty.is_non_null() {
                quote!(::core::option::Option::Some(value))
            } else {
                quote!(value @ ::core::option::Option::Some(_))
            };
            setters.push(quote! {
                let operation = match variables.#ident {
                    #value => operation.__variable(#key, value),
                    ::core::option::Option::None => operation,
                };
            });
        }
    }

    // `Default` only when it cannot leave out a required variable;
    // `InputObject` has no `PartialEq`.
    let mut derives = vec![
        quote!(::core::fmt::Debug),
        quote!(::core::clone::Clone),
        quote!(#krate::__private::serde::Serialize),
    ];
    if inputs.is_empty() {
        derives.push(quote!(::core::cmp::PartialEq));
    }
    if !operation.variables.iter().any(Variable::is_required) {
        derives.push(quote!(::core::default::Default));
    }
    let serde = serde_path(krate);
    let variables = Ident::new(VARIABLES_TYPE, Span::call_site());
    let struct_doc = format!(
        " The variables of the operation; those left to `None` are not sent. {runner} runs it."
    );
    let operation_doc = format!(" The operation with its variables, for {runner}.");
    quote! {
        #(#aliases)*

        #[doc = #struct_doc]
        #[derive(#(#derives),*)]
        #[serde(crate = #serde)]
        pub struct #variables {
            #(#fields)*
        }

        #[doc = #operation_doc]
        pub fn operation(variables: #variables) -> #target {
            let operation = #constructor;
            #(#setters)*
            operation
        }

        impl ::core::convert::From<#variables> for #target {
            fn from(variables: #variables) -> Self {
                operation(variables)
            }
        }
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
