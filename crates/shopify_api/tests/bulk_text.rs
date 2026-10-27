//! What the text of a bulk query holds, and how its lines are placed: the
//! values written into it, its strings, and the keys that route its lines.
#![cfg(feature = "testing")]

use shopify_api::testing::{MockTransport, client};
use shopify_api::{Error, InvalidValueReason, graphql, input};

const URL: &str = "https://storage.googleapis.com/shopify-tiers-assets-prod-us-east1/bulk-operation-outputs/text.jsonl";

graphql! {
    mod sorted = bulk("query($sort: ProductSortKeys) { products(sortKey: $sort) { edges { node { title } } } }");
    mod sorted_by_title = bulk("query($sort: ProductSortKeys = TITLE) { products(sortKey: $sort) { edges { node { title } } } }");
    mod topics = bulk("query($topics: [WebhookSubscriptionTopic!]) { webhookSubscriptions(topics: $topics) { edges { node { id } } } }");
    mod priced = bulk("query($context: ContextualPricingContext!) { products { edges { node { contextualPricing(context: $context) { priceRange { minVariantPrice { amount } } } } } } }");
    mod aliased_id = bulk("{ products { edges { node { id: totalInventory title } } } }");
    mod aliased_child_id = bulk("{ products { edges { node { id variants { edges { node { id: inventoryQuantity sku } } } } } } }");
    mod typenames = bulk("{ products { edges { node { id variants { edges { node { __typename kind: __typename sku } } } media { edges { node { __typename @include(if: false) alt } } } } } } }");
}

// ─── Enum values `true`, `false` and `null` ──────────────────────────────

fn refused_as_reserved(result: shopify_api::Result<String>, path: &str) {
    let error = result.unwrap_err();
    let Error::InvalidValue {
        path: at, reason, ..
    } = &error
    else {
        panic!("{error:?}");
    };
    assert_eq!(at, path);
    assert_eq!(*reason, InvalidValueReason::ReservedEnumValue);
}

#[test]
fn an_enum_variable_true_false_or_null_is_refused() {
    for word in ["null", "true", "false"] {
        let text = sorted::operation(sorted::Variables {
            sort: Some(word.into()),
        })
        .text();
        refused_as_reserved(text, "$sort");
    }
    // A real `null` is still `null`, and a name that only starts like one is a name.
    let text = graphql!(
        bulk("query($sort: ProductSortKeys) { products(sortKey: $sort) { edges { node { title } } } }"),
        sort = null,
    )
    .text()
    .unwrap();
    assert!(text.contains("products(sortKey: null)"), "{text}");
    let text = sorted::operation(sorted::Variables {
        sort: Some("nullable".into()),
    })
    .text()
    .unwrap();
    assert!(text.contains("products(sortKey: nullable)"), "{text}");
}

#[test]
fn a_none_in_variables_is_absence_and_null_is_null() {
    // `None` in `Variables` leaves the variable out: its argument goes, or
    // takes the variable's default.
    let text = sorted::operation(sorted::Variables { sort: None })
        .text()
        .unwrap();
    assert_eq!(text, "{ products { edges { node { title } } } }");
    let text = sorted_by_title::operation(sorted_by_title::Variables { sort: None })
        .text()
        .unwrap();
    assert!(text.contains("products(sortKey: TITLE)"), "{text}");
    // An explicit `null` is written, and replaces the default.
    let text = graphql!(
        bulk("query($sort: ProductSortKeys = TITLE) { products(sortKey: $sort) { edges { node { title } } } }"),
        sort = null,
    )
    .text()
    .unwrap();
    assert!(text.contains("products(sortKey: null)"), "{text}");
}

#[test]
fn an_enum_true_false_or_null_in_a_list_is_refused() {
    for word in ["null", "true", "false"] {
        let text = topics::operation(topics::Variables {
            topics: Some(vec!["ORDERS_CREATE".into(), word.into()]),
        })
        .text();
        refused_as_reserved(text, "$topics[1]");
    }
}

#[test]
fn an_enum_true_false_or_null_in_an_input_field_is_refused() {
    for text in [
        priced::operation(priced::Variables {
            context: input!(ContextualPricingContext {
                country: raw("null")
            }),
        })
        .text(),
        priced::operation(priced::Variables {
            context: input!(ContextualPricingContext {
                country: raw("true")
            }),
        })
        .text(),
        priced::operation(priced::Variables {
            context: input!(ContextualPricingContext {
                country: raw("false")
            }),
        })
        .text(),
    ] {
        refused_as_reserved(text, "$context.country");
    }
    let text = priced::operation(priced::Variables {
        context: input!(ContextualPricingContext { country: null }),
    })
    .text()
    .unwrap();
    assert!(
        text.contains("contextualPricing(context: {country: null})"),
        "{text}"
    );
}

// ─── Strings ─────────────────────────────────────────────────────────────

/// A GraphQL string token, quoted or block, read as the specification
/// reads it: a decoder of the test's own, not the crate's.
fn decode(token: &str) -> String {
    if let Some(body) = token.strip_prefix("\"\"\"") {
        let body = body.strip_suffix("\"\"\"").unwrap();
        let raw = body.replace("\\\"\"\"", "\"\"\"");
        let lines: Vec<String> = raw
            .replace("\r\n", "\n")
            .replace('\r', "\n")
            .split('\n')
            .map(str::to_string)
            .collect();
        let white = |line: &str| line.chars().take_while(|c| *c == ' ' || *c == '\t').count();
        let common = lines[1..]
            .iter()
            .filter(|line| white(line) < line.chars().count())
            .map(|line| white(line))
            .min();
        let mut lines: Vec<String> = lines
            .iter()
            .enumerate()
            .map(|(index, line)| match common {
                Some(common) if index > 0 => line.chars().skip(common).collect(),
                _ => line.clone(),
            })
            .collect();
        while lines
            .first()
            .is_some_and(|line| white(line) == line.chars().count())
        {
            lines.remove(0);
        }
        while lines
            .last()
            .is_some_and(|line| white(line) == line.chars().count())
        {
            lines.pop();
        }
        return lines.join("\n");
    }
    let body = token.strip_prefix('"').unwrap().strip_suffix('"').unwrap();
    let mut out = String::new();
    let mut chars = body.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            assert!(c != '\n' && c != '\r', "a line break in {token:?}");
            out.push(c);
            continue;
        }
        match chars.next().unwrap() {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            '/' => out.push('/'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'u' => {
                let hex: String = chars.by_ref().take(4).collect();
                let code = u32::from_str_radix(&hex, 16).unwrap();
                if (0xD800..0xDC00).contains(&code) {
                    // A surrogate pair.
                    assert_eq!(chars.next(), Some('\\'));
                    assert_eq!(chars.next(), Some('u'));
                    let low: String = chars.by_ref().take(4).collect();
                    let low = u32::from_str_radix(&low, 16).unwrap();
                    out.push(
                        char::from_u32(0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00)).unwrap(),
                    );
                } else {
                    out.push(char::from_u32(code).unwrap());
                }
            }
            other => panic!("unknown escape \\{other} in {token:?}"),
        }
    }
    out
}

/// The string token that follows `after` in `text`.
fn token_after<'t>(text: &'t str, after: &str) -> &'t str {
    let start = text
        .find(after)
        .unwrap_or_else(|| panic!("{after} in {text}"))
        + after.len();
    let rest = &text[start..];
    let mut escaped = false;
    for (index, c) in rest.char_indices().skip(1) {
        match c {
            '"' if !escaped => return &rest[..=index],
            _ => escaped = !escaped && c == '\\',
        }
    }
    panic!("an unterminated string in {text}");
}

/// The value Shopify reads for the string of `products(query: …)`.
fn sent(query: shopify_api::bulk::BulkQuery<impl Sized>) -> String {
    decode(token_after(&query.text().unwrap(), "products(query: "))
}

#[test]
fn every_escape_reaches_shopify_as_written() {
    // Each escape GraphQL defines, but `\b`, which `graphql!` refuses
    // (`tests/ui/bulk_string_escape.rs`): the value written, and the one
    // Shopify reads in the text sent, decoded by the test.
    let cases = [
        (
            sent(graphql!(bulk(
                r#"{ products(query: "a\"b") { edges { node { id } } } }"#
            ))),
            "a\"b",
        ),
        (
            sent(graphql!(bulk(
                r#"{ products(query: "a\\b") { edges { node { id } } } }"#
            ))),
            "a\\b",
        ),
        (
            sent(graphql!(bulk(
                r#"{ products(query: "a\/b") { edges { node { id } } } }"#
            ))),
            "a/b",
        ),
        (
            sent(graphql!(bulk(
                r#"{ products(query: "a\u0008b") { edges { node { id } } } }"#
            ))),
            "a\u{8}b",
        ),
        (
            sent(graphql!(bulk(
                r#"{ products(query: "a\fb") { edges { node { id } } } }"#
            ))),
            "a\u{c}b",
        ),
        (
            sent(graphql!(bulk(
                r#"{ products(query: "a\nb") { edges { node { id } } } }"#
            ))),
            "a\nb",
        ),
        (
            sent(graphql!(bulk(
                r#"{ products(query: "a\rb") { edges { node { id } } } }"#
            ))),
            "a\rb",
        ),
        (
            sent(graphql!(bulk(
                r#"{ products(query: "a\tb") { edges { node { id } } } }"#
            ))),
            "a\tb",
        ),
        (
            sent(graphql!(bulk(
                r#"{ products(query: "a\u00e9\u20AC\uFFFFb") { edges { node { id } } } }"#
            ))),
            "aé€\u{FFFF}b",
        ),
        (
            sent(graphql!(bulk(
                r#"{ products(query: "a\u0000\u001f\u007fb") { edges { node { id } } } }"#
            ))),
            "a\u{0}\u{1f}\u{7f}b",
        ),
        (
            sent(graphql!(bulk(
                r#"{ products(query: "é😀 \u2028 $x #y") { edges { node { id } } } }"#
            ))),
            "é😀 \u{2028} $x #y",
        ),
        (
            sent(graphql!(bulk(
                r#"query($filter: String = "a\"\\\/\u0008\f\n\r\t\u00e9b") { products(query: $filter) { edges { node { id } } } }"#
            ))),
            "a\"\\/\u{8}\u{c}\n\r\téb",
        ),
    ];
    for (index, (sent, written)) in cases.iter().enumerate() {
        assert_eq!(sent, written, "case {index}");
    }
}

#[test]
fn block_strings_reach_shopify_as_written() {
    let cases = [
        (
            sent(graphql!(bulk(
                r#"{ products(query: """plain""") { edges { node { id } } } }"#
            ))),
            "plain",
        ),
        (
            sent(graphql!(bulk(
                "{ products(query: \"\"\"\n    first\n      indented\n    last\n  \"\"\") { edges { node { id } } } }"
            ))),
            "first\n  indented\nlast",
        ),
        (
            sent(graphql!(bulk(
                r#"{ products(query: """a \""" b""") { edges { node { id } } } }"#
            ))),
            "a \"\"\" b",
        ),
        (
            sent(graphql!(bulk(
                r#"{ products(query: """no \n escape \u0041""") { edges { node { id } } } }"#
            ))),
            "no \\n escape \\u0041",
        ),
        (
            sent(graphql!(bulk(
                "{ products(query: \"\"\"\r\n\ta\r\n\t\tb\r\n\"\"\") { edges { node { id } } } }"
            ))),
            "a\n\tb",
        ),
        (
            sent(graphql!(bulk(
                "query($filter: String = \"\"\"\n  x\n  \"y\"\n\"\"\") { products(query: $filter) { edges { node { id } } } }"
            ))),
            "x\n\"y\"",
        ),
    ];
    for (index, (sent, written)) in cases.iter().enumerate() {
        assert_eq!(sent, written, "case {index}");
    }
}

#[test]
fn a_string_variable_reaches_shopify_as_given() {
    let value: String = (0..=0x10ffff).filter_map(char::from_u32).collect();
    let query = graphql!(
        bulk("query($filter: String) { products(query: $filter) { edges { node { title } } } }"),
        filter = value.clone(),
    );
    assert_eq!(sent(query), value);
}

// ─── The keys that place lines ───────────────────────────────────────────

#[tokio::test]
async fn an_item_with_an_alias_id_of_another_type_reads() {
    let mock = MockTransport::new();
    mock.push_chunks(
        200,
        [b"{\"id\":12,\"title\":\"Shirt\"}\n{\"id\":7,\"title\":\"Hat\"}\n".as_slice()],
    );
    let items = client(&mock)
        .download_bulk_items::<aliased_id::Item>(URL)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].id, 12);
    assert_eq!(items[0].title, "Shirt");
    assert_eq!(items[1].id, 7);
}

#[tokio::test]
async fn a_child_with_an_alias_id_of_another_type_reads() {
    let mock = MockTransport::new();
    mock.push_chunks(
        200,
        [b"{\"id\":\"P1\"}\n{\"id\":12,\"sku\":\"S-1\",\"__parentId\":\"P1\"}\n{\"id\":[1],\"sku\":\"S-2\",\"__parentId\":\"P1\"}\n{\"id\":\"P2\"}\n".as_slice()],
    );
    let items = client(&mock)
        .download_bulk_items::<aliased_child_id::Item>(URL)
        .await
        .unwrap()
        .collect()
        .await;
    // `[1]` is not an `Int`: that line alone fails, as its type says.
    let error = items.unwrap_err();
    assert!(matches!(error, Error::Decode { .. }), "{error:?}");

    let mock = MockTransport::new();
    mock.push_chunks(
        200,
        [b"{\"id\":\"P1\"}\n{\"id\":12,\"sku\":\"S-1\",\"__parentId\":\"P1\"}\n{\"id\":\"P2\"}\n".as_slice()],
    );
    let items = client(&mock)
        .download_bulk_items::<aliased_child_id::Item>(URL)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].variants[0].id, Some(12));
    assert_eq!(items[0].variants[0].sku.as_deref(), Some("S-1"));
    assert!(items[1].variants.is_empty());
}

#[tokio::test]
async fn lines_are_placed_by_the_reserved_key_not_by_the_query_s_typename() {
    let text = typenames::operation().text().unwrap();
    assert_eq!(
        text,
        "{ products { edges { node { id \
         variants { edges { node { _shopifyApiTypename: __typename __typename kind: __typename sku } } } \
         media { edges { node { _shopifyApiTypename: __typename __typename @include(if: false) alt } } } } } } }"
    );
    // The query's own `__typename` is absent where its `@include` left it
    // out, and says nothing of where a line goes.
    let mock = MockTransport::new();
    mock.push_chunks(200, [br#"{"id":"P1"}
{"_shopifyApiTypename":"ProductVariant","__typename":"ProductVariant","kind":"ProductVariant","sku":"S-1","__parentId":"P1"}
{"_shopifyApiTypename":"MediaImage","alt":"Front","__parentId":"P1"}
"#
    .as_slice()]);
    let items = client(&mock)
        .download_bulk_items::<typenames::Item>(URL)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap();
    assert_eq!(items[0].variants[0].typename, "ProductVariant");
    assert_eq!(items[0].variants[0].kind, "ProductVariant");
    assert_eq!(items[0].media[0].typename, None);
    assert_eq!(items[0].media[0].alt.as_deref(), Some("Front"));

    // A `__typename` alone does not place a line.
    let mock = MockTransport::new();
    mock.push_chunks(
        200,
        [br#"{"id":"P1"}
{"__typename":"ProductVariant","kind":"ProductVariant","sku":"S-1","__parentId":"P1"}
"#
        .as_slice()],
    );
    let error = client(&mock)
        .download_bulk_items::<typenames::Item>(URL)
        .await
        .unwrap()
        .collect()
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "bulk operation: line 2 of the results has no `_shopifyApiTypename`, the `__typename` that tells which connection of its parent it belongs to"
    );
}
