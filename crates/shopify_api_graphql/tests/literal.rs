//! Values written in Rust for input types, checked against the Shopify Admin
//! schema 2026-10: what is refused, with which message, and how what is
//! accepted is sent.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use proc_macro2::{Ident, Span};
use quote::quote;
use shopify_api_graphql::codegen::input_object_id;
use shopify_api_graphql::literal::{
    Expr, Field, For, List, Literals, Object, Value, VariableValue,
};
use shopify_api_graphql::{Schema, TypeDef, analyze};

fn admin() -> &'static Schema {
    static ADMIN: OnceLock<Schema> = OnceLock::new();
    ADMIN.get_or_init(|| {
        Schema::from_json(include_str!("../../shopify_api/graphql/admin.schema.json")).unwrap()
    })
}

fn rust(text: &str) -> Value {
    Value::Rust(Expr {
        tokens: text.parse().unwrap(),
        span: Span::call_site(),
    })
}

fn ident(name: &str) -> Value {
    Value::Ident(Ident::new(name, Span::call_site()))
}

fn null() -> Value {
    Value::Null(Span::call_site())
}

fn raw(text: &str) -> Value {
    Value::Raw {
        keyword: Span::call_site(),
        value: Expr {
            tokens: text.parse().unwrap(),
            span: Span::call_site(),
        },
    }
}

/// An object; a name ending with `?` is `name?: value`.
fn object(fields: Vec<(&str, Value)>) -> Object {
    Object {
        span: Span::call_site(),
        fields: fields
            .into_iter()
            .map(|(name, value)| Field {
                name: name.trim_end_matches('?').to_string(),
                name_span: Span::call_site(),
                optional: name.ends_with('?').then(Span::call_site),
                value,
            })
            .collect(),
    }
}

fn list(items: Vec<Value>) -> Value {
    Value::List(List {
        span: Span::call_site(),
        items,
        rust: None,
    })
}

/// The code of `input!(name { … })` and, after it, its warnings, or the
/// refusal.
fn input(name: &str, fields: Vec<(&str, Value)>) -> Result<(String, String), String> {
    let mut literals = Literals::new(admin(), quote!(::shopify_api));
    let code = literals
        .input_object(name, Span::call_site(), &object(fields))
        .map_err(|error| error.message)?;
    let all = format!("{code} {}", literals.finish());
    Ok((code.to_string(), all))
}

fn refusal(name: &str, fields: Vec<(&str, Value)>) -> String {
    match input(name, fields) {
        Ok((code, _)) => panic!("accepted: {code}"),
        Err(message) => message,
    }
}

#[test]
fn an_object_is_sent_field_by_field_in_its_order() {
    let (code, items) = input(
        "ProductUpdateInput",
        vec![
            ("id", rust("product_id")),
            ("title", rust("format!(\"{name} (promo)\")")),
            ("status", ident("ACTIVE")),
            ("descriptionHtml", null()),
            ("vendor?", rust("new_vendor")),
            (
                "seo",
                Value::Object(object(vec![("title", rust("seo_title"))])),
            ),
            (
                "metafields",
                Value::For(Box::new(For {
                    span: Span::call_site(),
                    pattern: quote!(m),
                    binding: Some(Ident::new("m", Span::call_site())),
                    iterable: quote!(&rows),
                    filter: Some(quote!(!m.value.is_empty())),
                    item: Value::Object(object(vec![
                        ("key", rust("&m.key")),
                        ("type", rust("\"single_line_text_field\"")),
                    ])),
                })),
            ),
            ("tags", list(vec![rust("\"a\""), rust("tag")])),
        ],
    )
    .unwrap();
    let id = input_object_id("ProductUpdateInput");
    assert!(
        code.starts_with(&format!(
            ":: shopify_api :: InputObject :: < {id}u64 > :: __new (\"ProductUpdateInput\" , & :: shopify_api :: __private :: literal :: Object ((:: shopify_api :: __private :: literal :: Field (\"id\" , ({{"
        )),
        "{code}"
    );
    let keys: Vec<&str> = code
        .match_indices("literal :: Field (\"")
        .chain(code.match_indices("literal :: Optional (\""))
        .map(|(at, matched)| {
            let rest = &code[at + matched.len()..];
            &rest[..rest.find('"').unwrap()]
        })
        .collect();
    assert_eq!(
        keys,
        [
            "id",
            "title",
            "status",
            "descriptionHtml",
            "seo",
            "title",
            "metafields",
            "key",
            "type",
            "tags",
            "vendor"
        ]
    );
    // The enum value is a string; `null` is `()`; the list is built in a loop.
    assert!(code.contains("Field (\"status\" , \"ACTIVE\")"), "{code}");
    assert!(code.contains("Field (\"descriptionHtml\" , ())"), "{code}");
    assert!(
        code.contains("for m in & rows { if ! m . value . is_empty ()"),
        "{code}"
    );
    // Each Rust expression goes through a trait that names its place.
    assert!(
        items.contains(
            "message = \"`title` of `ProductUpdateInput` expects a `String`, not `{Self}`\""
        ),
        "{items}"
    );
    assert!(
        items.contains("message = \"`vendor?:` takes an `Option` or a `shopify_api::Maybe` of a `String`, not `{Self}`\""),
        "{items}"
    );
    assert!(
        items.contains("message = \"`vendor` of `ProductUpdateInput` takes a `String`, not an `Option`: write `vendor?: value` to leave the field out when it is `None`, or `vendor: null` to send `null`\""),
        "{items}"
    );
}

#[test]
fn fields_are_checked_against_the_input_object() {
    assert_eq!(
        refusal(
            "ProductUpdateInput",
            vec![("id", rust("id")), ("titel", rust("title"))]
        ),
        "Input object `ProductUpdateInput` has no field `titel` (did you mean `title`?)"
    );
    assert_eq!(
        refusal(
            "ProductUpdateInput",
            vec![("title", rust("a")), ("title", rust("b"))]
        ),
        "The field `title` of `ProductUpdateInput` is given twice"
    );
    assert_eq!(
        refusal(
            "MetafieldsSetInput",
            vec![("namespace", rust("\"custom\""))]
        ),
        "Input object `MetafieldsSetInput` requires the fields `key: String!`, `ownerId: ID!` and `value: String!`: add them"
    );
    assert_eq!(
        refusal(
            "MetafieldsSetInput",
            vec![("ownerId", rust("id")), ("key", rust("key"))]
        ),
        "Input object `MetafieldsSetInput` requires the field `value: String!`: add it"
    );
    // The fields of a nested object, and of the items of a list.
    assert_eq!(
        refusal(
            "ProductUpdateInput",
            vec![(
                "metafields",
                list(vec![Value::Object(object(vec![("kee", rust("k"))]))])
            )]
        ),
        "Input object `MetafieldInput` has no field `kee` (did you mean `key`?)"
    );
}

#[test]
fn null_and_absence_follow_nullability_and_defaults() {
    assert_eq!(
        refusal(
            "MetafieldsSetInput",
            vec![
                ("ownerId", null()),
                ("key", rust("k")),
                ("value", rust("v"))
            ]
        ),
        "`null` is not allowed for `ownerId` of `MetafieldsSetInput`, of type `ID!`"
    );
    assert_eq!(
        refusal(
            "MetafieldsSetInput",
            vec![
                ("ownerId?", rust("id")),
                ("key", rust("k")),
                ("value", rust("v"))
            ]
        ),
        "`ownerId` of `MetafieldsSetInput` is required (`ID!`): it cannot be left out, so `?:` does not apply; write `ownerId: …`"
    );
    assert_eq!(
        refusal("ProductUpdateInput", vec![("vendor?", null())]),
        "`vendor?: null` is `vendor: null`"
    );
    assert_eq!(
        refusal(
            "ProductUpdateInput",
            vec![("seo?", Value::Object(object(vec![])))]
        ),
        "`seo?:` takes a Rust expression, an `Option` or a `shopify_api::Maybe` whose `None` leaves the field out; write `seo: …` for this value"
    );
    // Non-null with a default: `?:` takes an `Option`, not a `Maybe`.
    let (_, items) = input(
        "InventoryTransferCreateInput",
        vec![("lineItems?", rust("line_items"))],
    )
    .unwrap();
    assert!(
        items.contains("`lineItems` of `InventoryTransferCreateInput` is non-null (`[InventoryTransferLineItemInput!]!`): `lineItems?:` takes an `Option`, not a `Maybe`, whose `Null` it refuses"),
        "{items}"
    );
    assert!(
        items.contains("message = \"`lineItems?:` takes an `Option` of a list of `InventoryTransferLineItemInput`, built with `input!(InventoryTransferLineItemInput {{ … }})`, not `{Self}`\""),
        "{items}"
    );
}

#[test]
fn enum_values_are_checked_and_raw_is_only_for_enums() {
    assert_eq!(
        refusal("ProductUpdateInput", vec![("status", ident("ACTIV"))]),
        "`ACTIV` is not a value of the enum `ProductStatus` (did you mean `ACTIVE`?): `status` of `ProductUpdateInput` takes `ACTIVE`, `ARCHIVED`, `DRAFT` or `UNLISTED`"
    );
    assert_eq!(
        refusal("ProductUpdateInput", vec![("status", ident("status"))]),
        "`status` is not a value of the enum `ProductStatus`: `status` of `ProductUpdateInput` takes `ACTIVE`, `ARCHIVED`, `DRAFT` or `UNLISTED`; for a string known at run time, write `raw(status)`"
    );
    assert_eq!(
        refusal("ProductUpdateInput", vec![("status", rust("\"ACTIVE\""))]),
        "`status` of `ProductUpdateInput` expects a value of the enum `ProductStatus`: write it bare, as `ACTIVE`, or `raw(…)` for a string known at run time"
    );
    assert_eq!(
        refusal("ProductUpdateInput", vec![("status?", rust("status"))]),
        "`status` of `ProductUpdateInput` holds a value of the enum `ProductStatus`: for a string known at run time, write `status?: raw(…)`, with an `Option` of it"
    );
    assert_eq!(
        refusal("ProductUpdateInput", vec![("title", raw("title"))]),
        "`raw(…)` is only for a value of an enum: `title` of `ProductUpdateInput` expects a `String`"
    );
    assert_eq!(
        refusal(
            "CollectionSourceInclusionConditionProductStatusInput",
            vec![("values", rust("statuses"))]
        ),
        "`values` of `CollectionSourceInclusionConditionProductStatusInput` expects a list of values of the enum `ProductStatus`: write them bare, `[ACTIVE, …]`, or `[for value in … => raw(value)]` for strings known at run time"
    );
    // A long enum is listed in part.
    let exemptions = refusal(
        "CompanyLocationInput",
        vec![
            ("name", rust("name")),
            ("taxExemptions", list(vec![ident("NONE")])),
        ],
    );
    assert!(
        exemptions.starts_with("`NONE` is not a value of the enum `TaxExemption`: an item of `taxExemptions` of `CompanyLocationInput` takes `CA_STATUS_CARD_EXEMPTION`, "),
        "{exemptions}"
    );
    assert!(exemptions.ends_with(" values)"), "{exemptions}");

    let (code, _) = input(
        "ProductUpdateInput",
        vec![("status", raw("status")), ("id", rust("id"))],
    )
    .unwrap();
    assert!(
        code.contains(
            "Field (\"status\" , :: shopify_api :: __private :: literal :: raw (status))"
        ),
        "{code}"
    );
}

#[test]
fn a_value_has_the_form_of_its_type() {
    assert_eq!(
        refusal(
            "ProductUpdateInput",
            vec![("title", Value::Object(object(vec![("value", rust("v"))])))]
        ),
        "`title` of `ProductUpdateInput` expects a `String`, not an object"
    );
    assert_eq!(
        refusal("ProductUpdateInput", vec![("title", list(vec![rust("t")]))]),
        "`title` of `ProductUpdateInput` expects a `String`, not a list"
    );
    // No single value where a list is expected.
    assert_eq!(
        refusal(
            "ProductUpdateInput",
            vec![("metafields", Value::Object(object(vec![])))]
        ),
        "`metafields` of `ProductUpdateInput` expects a list of `MetafieldInput`, built with `input!(MetafieldInput { … })`, not an object"
    );
    assert_eq!(
        refusal(
            "ProductCreateInput",
            vec![("productOptions", list(vec![null()]))]
        ),
        "`null` is not allowed for an item of `productOptions` of `ProductCreateInput`, of type `OptionCreateInput!`"
    );
}

#[test]
fn input_takes_an_input_object_of_the_schema() {
    assert_eq!(
        refusal("MetafieldSetInput", vec![]),
        "The schema has no input object `MetafieldSetInput` (did you mean `MetafieldsSetInput`?)"
    );
    assert_eq!(
        refusal("Product", vec![]),
        "`Product` is an object, not an input object: `input!` builds the input objects of the schema"
    );
}

#[test]
fn deprecated_fields_and_enum_values_warn() {
    let (_, items) = input("ProductVariantsBulkInput", vec![("barcode", rust("code"))]).unwrap();
    assert!(
        items.contains("deprecated (note = \"`ProductVariantsBulkInput.barcode` is deprecated in the Shopify Admin API 2026-10: Use `barcodes` instead.\")"),
        "{items}"
    );
    let (_, items) = input(
        "DeliveryCarrierCalculatedOptionDefinitionUpdateInput",
        vec![("id", rust("id")), ("currency", ident("ANG"))],
    )
    .unwrap();
    assert!(
        items.contains("deprecated (note = \"`CurrencyCode.ANG` is deprecated in the Shopify Admin API 2026-10: Use `XCG` instead.\")"),
        "{items}"
    );
}

fn variable(source: &str, value: Value) -> Result<VariableValue, String> {
    let operation = analyze(admin(), source).unwrap();
    let mut literals = Literals::new(admin(), quote!(::shopify_api));
    literals
        .variable(&operation.variables[0], &value)
        .map_err(|error| error.message)
}

#[test]
fn inline_variables_of_scalars_convert_as_before_and_others_are_literals() {
    let converted = |value: VariableValue| match value {
        VariableValue::Converted { ty, value } => format!("{ty} = {}", value.tokens),
        VariableValue::Literal(code) => panic!("a literal: {code}"),
    };
    let ids = "query($ids: [ID!]!) { nodes(ids: $ids) { id } }";
    assert_eq!(
        converted(
            variable(
                "query($first: Int!) { products(first: $first) { nodes { id } } }",
                rust("{ compute() }")
            )
            .unwrap()
        ),
        ":: core :: primitive :: i64 = { compute () }"
    );
    let array = Value::List(List {
        span: Span::call_site(),
        items: vec![ident("a"), rust("&b")],
        rust: Some(Expr {
            tokens: quote!([a, &b]),
            span: Span::call_site(),
        }),
    });
    assert_eq!(
        converted(variable(ids, array).unwrap()),
        ":: std :: vec :: Vec < :: std :: string :: String > = [a , & b]"
    );
    let sort = "query($sortKey: ProductSortKeys) { products(first: 1, sortKey: $sortKey) { nodes { id } } }";
    assert_eq!(
        converted(variable(sort, raw("key")).unwrap()),
        ":: core :: option :: Option < :: std :: string :: String > = key"
    );
    assert!(matches!(
        variable(sort, ident("TITLE")).unwrap(),
        VariableValue::Literal(_)
    ));
    assert_eq!(
        variable(sort, rust("key")).unwrap_err(),
        "`$sortKey` expects a value of the enum `ProductSortKeys`: write it bare, as `CREATED_AT`, or `raw(…)` for a string known at run time"
    );
    assert_eq!(
        variable(ids, raw("id")).unwrap_err(),
        "`raw(…)` is only for a value of an enum: `$ids` expects a list of `ID`"
    );
    let metafields = "mutation($metafields: [MetafieldsSetInput!]!) { metafieldsSet(metafields: $metafields) { userErrors { message } } }";
    let VariableValue::Literal(code) = variable(metafields, rust("rows")).unwrap() else {
        panic!("a literal");
    };
    assert!(
        code.to_string().contains("Checks }) . check ((rows))"),
        "{code}"
    );
    assert_eq!(
        variable(metafields, Value::Object(object(vec![]))).unwrap_err(),
        "`$metafields` expects a list of `MetafieldsSetInput`, built with `input!(MetafieldsSetInput { … })`, not an object"
    );
}

#[test]
fn input_object_ids_differ_for_every_input_object_of_the_schema() {
    // FNV-1a: the same in every crate and every build.
    assert_eq!(input_object_id(""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(input_object_id("a"), 0xaf63_dc4c_8601_ec8c);
    let mut seen = BTreeMap::new();
    for (name, definition) in &admin().types {
        if let TypeDef::Input { .. } = definition
            && let Some(other) = seen.insert(input_object_id(name), name)
        {
            panic!("`{name}` and `{other}` have the same id");
        }
    }
    assert!(seen.len() > 500);
}
