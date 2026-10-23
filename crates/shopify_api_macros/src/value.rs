//! Reads the values of inline variables and of `input!` into
//! [`Value`]s, which `shopify_api_graphql::literal` checks against the
//! schema.
//!
//! The text alone decides what is read; the type expected where the value
//! stands decides what it means:
//!
//! - `{` starts an object when it is empty or starts with `name:` or
//!   `name?:`, which no Rust block does; `{ compute() }` stays a block.
//! - `[a, b]` is kept both as a list of values and as the Rust array it
//!   also is: a list where a list is expected, the array elsewhere. `[x; 3]`
//!   is an array.
//! - `null`, `raw(…)` and a lone identifier are those forms only when they
//!   are the whole value: `null.clone()` or `raw(x).len()` are Rust.

use proc_macro2::{Delimiter, Spacing, Span, TokenTree};
use quote::ToTokens;
use shopify_api_graphql::literal::{Expr, Field, For, List, Object, Value};
use syn::buffer::Cursor;
use syn::ext::IdentExt;
use syn::parse::{ParseStream, Parser};
use syn::spanned::Spanned;
use syn::{Ident, Token};

pub(crate) fn parse_value(input: ParseStream) -> syn::Result<Value> {
    let cursor = input.cursor();
    if let Some((ident, rest)) = cursor.ident() {
        if ident == "null" && ends_after(input, 1) {
            input.call(Ident::parse_any)?;
            return Ok(Value::Null(ident.span()));
        }
        if ident == "raw" && rest.group(Delimiter::Parenthesis).is_some() && ends_after(input, 2) {
            input.call(Ident::parse_any)?;
            let content;
            syn::parenthesized!(content in input);
            let value: syn::Expr = content.parse().map_err(|error| {
                syn::Error::new(
                    error.span(),
                    "`raw(…)` takes the value of the enum, as a string",
                )
            })?;
            if !content.is_empty() {
                return Err(content.error("`raw(…)` takes one value"));
            }
            return Ok(Value::Raw {
                keyword: ident.span(),
                value: expr(&value),
            });
        }
        if ends_after(input, 1) {
            return Ok(Value::Ident(input.call(Ident::parse_any)?));
        }
    }
    if let Some((inside, _, _)) = cursor.group(Delimiter::Brace)
        && ends_after(input, 1)
        && is_object(inside)
    {
        let content;
        let braces = syn::braced!(content in input);
        return parse_object(&content, braces.span.join());
    }
    if cursor.group(Delimiter::Bracket).is_some() && ends_after(input, 1) {
        let Some(TokenTree::Group(group)) = input.parse::<Option<TokenTree>>()? else {
            unreachable!("a bracketed group was seen");
        };
        return parse_brackets(group);
    }
    let value: syn::Expr = input.parse()?;
    Ok(Value::Rust(expr(&value)))
}

/// `{ name: value, name?: value, … }`, its braces read.
pub(crate) fn parse_object(content: ParseStream, span: Span) -> syn::Result<Value> {
    let mut fields = Vec::new();
    while !content.is_empty() {
        let name = content
            .call(Ident::parse_any)
            .map_err(|error| syn::Error::new(error.span(), "expected a field: `name: value`"))?;
        let optional = content
            .parse::<Option<Token![?]>>()?
            .map(|question| question.span);
        if !content.peek(Token![:]) || content.peek(Token![::]) {
            return Err(syn::Error::new(
                name.span(),
                format!("expected `:` after the field: `{name}: value`"),
            ));
        }
        content.parse::<Token![:]>()?;
        let value = parse_value(content)?;
        fields.push(Field {
            name: name.unraw().to_string(),
            name_span: name.span(),
            optional,
            value,
        });
        if content.is_empty() {
            break;
        }
        content
            .parse::<Token![,]>()
            .map_err(|error| syn::Error::new(error.span(), "expected `,` between two fields"))?;
    }
    Ok(Value::Object(Object { span, fields }))
}

fn parse_brackets(group: proc_macro2::Group) -> syn::Result<Value> {
    let span = group.span();
    let content = group.stream();
    if matches!(content.clone().into_iter().next(), Some(TokenTree::Ident(ident)) if ident == "for")
    {
        return (|input: ParseStream| parse_for(input, span)).parse2(content);
    }
    let items = (|input: ParseStream| {
        let mut items = Vec::new();
        while !input.is_empty() {
            items.push(parse_value(input)?);
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        Ok(items)
    })
    .parse2(content);
    let rust = syn::parse2::<syn::Expr>(TokenTree::Group(group).into_token_stream())
        .ok()
        .map(|array| expr(&array));
    match (items, rust) {
        (Ok(items), rust) => Ok(Value::List(List { span, items, rust })),
        (Err(_), Some(rust)) => Ok(Value::Rust(rust)),
        (Err(error), None) => Err(error),
    }
}

/// `for pattern in iterable if filter => item`, inside the brackets.
fn parse_for(input: ParseStream, span: Span) -> syn::Result<Value> {
    input.parse::<Token![for]>()?;
    let pattern = syn::Pat::parse_multi_with_leading_vert(input)?;
    input.parse::<Token![in]>()?;
    let iterable = syn::Expr::parse_without_eager_brace(input)?;
    let filter = match input.parse::<Option<Token![if]>>()? {
        Some(_) => Some(syn::Expr::parse_without_eager_brace(input)?.into_token_stream()),
        None => None,
    };
    input.parse::<Token![=>]>().map_err(|error| {
        syn::Error::new(
            error.span(),
            "expected `=>` and the value of each item: `[for x in xs => value]`",
        )
    })?;
    let item = parse_value(input)?;
    if !input.is_empty() {
        return Err(input.error("expected `]`: one value follows `=>`"));
    }
    Ok(Value::For(Box::new(For {
        span,
        pattern: pattern.into_token_stream(),
        iterable: iterable.into_token_stream(),
        filter,
        item,
    })))
}

fn expr(value: &syn::Expr) -> Expr {
    Expr {
        tokens: value.to_token_stream(),
        span: value.span(),
    }
}

/// The value ends after `count` token trees: at the end, or at a `,`.
fn ends_after(input: ParseStream, count: usize) -> bool {
    let mut cursor = input.cursor();
    for _ in 0..count {
        match cursor.token_tree() {
            Some((_, next)) => cursor = next,
            None => return false,
        }
    }
    cursor.eof() || matches!(cursor.punct(), Some((punct, _)) if punct.as_char() == ',')
}

/// The inside of braces reads as an object: empty, or `name:` (not
/// `name::`) or `name?:` first.
fn is_object(inside: Cursor) -> bool {
    if inside.eof() {
        return true;
    }
    let Some((_, rest)) = inside.ident() else {
        return false;
    };
    let is = |cursor: Cursor, character: char| matches!(cursor.punct(), Some((punct, _)) if punct.as_char() == character);
    match rest.punct() {
        Some((colon, after)) if colon.as_char() == ':' => {
            !(colon.spacing() == Spacing::Joint && is(after, ':'))
        }
        Some((question, after)) if question.as_char() == '?' => is(after, ':'),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Value {
        parse_value.parse_str(text).unwrap()
    }

    fn kind(value: &Value) -> String {
        match value {
            Value::Null(_) => "null".to_string(),
            Value::Raw { value, .. } => format!("raw({})", value.tokens),
            Value::Ident(ident) => format!("ident {ident}"),
            Value::Object(object) => {
                let fields: Vec<String> = object
                    .fields
                    .iter()
                    .map(|field| {
                        let question = if field.optional.is_some() { "?" } else { "" };
                        format!("{}{question}: {}", field.name, kind(&field.value))
                    })
                    .collect();
                format!("{{{}}}", fields.join(", "))
            }
            Value::List(list) => {
                let items: Vec<String> = list.items.iter().map(kind).collect();
                let rust = if list.rust.is_some() {
                    " (also rust)"
                } else {
                    ""
                };
                format!("[{}]{rust}", items.join(", "))
            }
            Value::For(comprehension) => format!(
                "for {} in {}{} => {}",
                comprehension.pattern,
                comprehension.iterable,
                comprehension
                    .filter
                    .as_ref()
                    .map(|filter| format!(" if {filter}"))
                    .unwrap_or_default(),
                kind(&comprehension.item)
            ),
            Value::Rust(expr) => format!("rust {}", expr.tokens),
        }
    }

    #[test]
    fn reads_each_form_from_the_text_alone() {
        for (text, read) in [
            ("null", "null"),
            ("null.clone()", "rust null . clone ()"),
            ("raw(status)", "raw(status)"),
            ("raw(a).len()", "rust raw (a) . len ()"),
            ("ACTIVE", "ident ACTIVE"),
            ("{}", "{}"),
            ("{ compute() }", "rust { compute () }"),
            ("{ x }", "rust { x }"),
            ("{ a::b() }", "rust { a :: b () }"),
            (
                "{ id: id, type: \"t\", vendor?: v, seo: { title: t }, }",
                "{id: ident id, type: rust \"t\", vendor?: ident v, seo: {title: ident t}}",
            ),
            ("[a, b]", "[ident a, ident b] (also rust)"),
            ("[x; 3]", "rust [x ; 3]"),
            ("[{ a: 1 }, null]", "[{a: rust 1}, null]"),
            ("[a, b].to_vec()", "rust [a , b] . to_vec ()"),
            (
                "[for m in &rows if !m.value.is_empty() => { key: &m.key }]",
                "for m in & rows if ! m . value . is_empty () => {key: rust & m . key}",
            ),
            (
                "[for (i, m) in rows.iter().enumerate() => i]",
                "for (i , m) in rows . iter () . enumerate () => ident i",
            ),
            ("format!(\"{name}\")", "rust format ! (\"{name}\")"),
            ("Some(x)", "rust Some (x)"),
        ] {
            assert_eq!(kind(&parse(text)), read, "{text}");
        }
    }

    #[test]
    fn says_what_is_wrong_in_an_object() {
        for (text, message) in [
            ("{ a: 1 b: 2 }", "expected `,` between two fields"),
            ("{ a: 1, b }", "expected `:` after the field: `b: value`"),
            ("{ a: 1, 2: 3 }", "expected a field: `name: value`"),
            ("raw()", "`raw(…)` takes the value of the enum, as a string"),
            ("raw(a, b)", "`raw(…)` takes one value"),
            (
                "[for x in xs x]",
                "expected `=>` and the value of each item: `[for x in xs => value]`",
            ),
            (
                "[for x in xs => x, y]",
                "expected `]`: one value follows `=>`",
            ),
        ] {
            let error = parse_value.parse_str(text).expect_err(text);
            assert_eq!(error.to_string(), message, "{text}");
        }
    }
}
