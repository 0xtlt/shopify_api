//! Values written in Rust for input types: object literals in inline
//! operations, `input!`, and the `Variables` of modules. Each test reads the
//! exact body sent.
#![cfg(feature = "testing")]

use serde_json::json;
use shopify_api::testing::{MockTransport, client};
use shopify_api::{Maybe, graphql, input};

const PRODUCT_UPDATE: &str = "mutation($product: ProductUpdateInput!) { productUpdate(product: $product) { userErrors { field message } } }";

graphql! {
    /// Writes metafields.
    pub mod metafields_set = "mutation MetafieldsSet($metafields: [MetafieldsSetInput!]!) {
        metafieldsSet(metafields: $metafields) { metafields { id key } userErrors { field message } }
    }";
    mod shop_name = "query ShopName { shop { name } }";
    pub(crate) mod recent_products = "query RecentProducts($first: Int!, $sortKey: ProductSortKeys) {
        products(first: $first, sortKey: $sortKey) { nodes { id } }
    }";
    mod product_count = "query ProductCount($query: String) { productsCount(query: $query) { count } }"
}

/// The body of the last request, as sent.
fn sent(mock: &MockTransport) -> String {
    String::from_utf8(mock.last_request().unwrap().body).unwrap()
}

/// A body of `query` with `variables`, written as `shopify_api` writes it.
fn body(query: &str, variables: &str) -> String {
    format!(
        r#"{{"query":{},"variables":{variables}}}"#,
        serde_json::to_string(query).unwrap()
    )
}

struct Row {
    product_id: String,
    key: &'static str,
    value: &'static str,
    digest: Option<String>,
}

fn rows() -> Vec<Row> {
    let row = |id: u32, key, value, digest: Option<&str>| Row {
        product_id: format!("gid://shopify/Product/{id}"),
        key,
        value,
        digest: digest.map(str::to_string),
    };
    vec![
        row(1, "color", "blue", None),
        row(2, "color", "", None),
        row(3, "color", "red", Some("d3")),
    ]
}

fn color_metafields(rows: &[Row]) -> Vec<metafields_set::MetafieldsSetInput> {
    let mut metafields = Vec::new();
    for row in rows {
        if row.value.is_empty() {
            continue;
        }
        metafields.push(input!(MetafieldsSetInput {
            ownerId: &row.product_id,
            namespace: "custom",
            key: row.key,
            value: row.value,
            type: "single_line_text_field",
            compareDigest?: row.digest.as_deref(),
        }));
    }
    metafields
}

const COLOR_METAFIELDS: &str = r#"[{"ownerId":"gid://shopify/Product/1","namespace":"custom","key":"color","value":"blue","type":"single_line_text_field"},{"ownerId":"gid://shopify/Product/3","namespace":"custom","key":"color","value":"red","type":"single_line_text_field","compareDigest":"d3"}]"#;

#[tokio::test]
async fn an_object_literal_sends_values_nulls_enums_nested_objects_and_built_lists() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "productUpdate": { "userErrors": [] } }));
    let shopify = client(&mock);
    let product_id = "gid://shopify/Product/1";
    let name = "Shirt";
    let new_vendor: Option<String> = None;
    let seo_title = "Shirt, on sale".to_string();
    let rows = rows();

    let data = shopify
        .execute(graphql!(
            "mutation($product: ProductUpdateInput!) { productUpdate(product: $product) { userErrors { field message } } }",
            product = {
                id: product_id,
                title: format!("{name} (promo)"),
                status: ACTIVE,
                descriptionHtml: null,
                vendor?: new_vendor,
                seo: { title: &seo_title },
                metafields: [for m in &rows if !m.value.is_empty() => {
                    namespace: "custom", key: m.key, value: m.value, type: "single_line_text_field",
                }],
            },
        ))
        .await
        .unwrap();

    assert!(data.product_update.unwrap().user_errors.is_empty());
    assert_eq!(
        sent(&mock),
        body(
            PRODUCT_UPDATE,
            r#"{"product":{"id":"gid://shopify/Product/1","title":"Shirt (promo)","status":"ACTIVE","descriptionHtml":null,"seo":{"title":"Shirt, on sale"},"metafields":[{"namespace":"custom","key":"color","value":"blue","type":"single_line_text_field"},{"namespace":"custom","key":"color","value":"red","type":"single_line_text_field"}]}}"#
        )
    );
}

#[tokio::test]
async fn a_field_with_a_question_mark_is_left_out_sent_null_or_sent() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "productUpdate": { "userErrors": [] } }));
    let shopify = client(&mock);
    let clear_description = true;
    let status: Option<String> = Some("DRAFT".to_string());

    shopify
        .execute(graphql!(
            "mutation($product: ProductUpdateInput!) { productUpdate(product: $product) { userErrors { field message } } }",
            product = {
                id: "gid://shopify/Product/1",
                vendor?: Some("Acme"),
                handle?: None::<String>,
                descriptionHtml?: if clear_description { Maybe::Null } else { Maybe::Value("<p>x</p>") },
                productType?: Maybe::Value("Shirts".to_string()),
                templateSuffix?: Maybe::<&str>::Absent,
                giftCardTemplateSuffix?: Maybe::<String>::default(),
                status?: raw(status.as_deref()),
                tags: ["a", "b"],
                collectionsToJoin: [],
            },
        ))
        .await
        .unwrap();

    assert_eq!(
        sent(&mock),
        body(
            PRODUCT_UPDATE,
            r#"{"product":{"id":"gid://shopify/Product/1","vendor":"Acme","descriptionHtml":null,"productType":"Shirts","status":"DRAFT","tags":["a","b"],"collectionsToJoin":[]}}"#
        )
    );
}

#[tokio::test]
async fn enum_variables_take_a_bare_value_or_raw_and_scalars_take_rust_as_before() {
    let mock = MockTransport::new();
    mock.respond_with(|_| {
        Ok(shopify_api::transport::HttpResponse::json(
            200,
            &json!({ "data": { "products": { "nodes": [] }, "nodes": [] } }),
        ))
    });
    let shopify = client(&mock);
    let query = "query($first: Int!, $sortKey: ProductSortKeys, $ids: [ID!]!) { products(first: $first, sortKey: $sortKey) { nodes { id } } nodes(ids: $ids) { id } }";
    let (a, b) = (
        "gid://shopify/Product/1",
        "gid://shopify/Product/2".to_string(),
    );

    shopify
        .execute(graphql!(
            "query($first: Int!, $sortKey: ProductSortKeys, $ids: [ID!]!) { products(first: $first, sortKey: $sortKey) { nodes { id } } nodes(ids: $ids) { id } }",
            first = { 1 + 1 },
            sortKey = TITLE,
            ids = [a, &b],
        ))
        .await
        .unwrap();
    assert_eq!(
        sent(&mock),
        body(
            query,
            r#"{"first":2,"sortKey":"TITLE","ids":["gid://shopify/Product/1","gid://shopify/Product/2"]}"#
        )
    );

    let sort_key: Option<&str> = None;
    shopify
        .execute(graphql!(
            "query($first: Int!, $sortKey: ProductSortKeys, $ids: [ID!]!) { products(first: $first, sortKey: $sortKey) { nodes { id } } nodes(ids: $ids) { id } }",
            first = 1_u8,
            sortKey = raw(sort_key),
            ids = [for id in [a, b.as_str()] => id],
        ))
        .await
        .unwrap();
    assert_eq!(
        sent(&mock),
        body(
            query,
            r#"{"first":1,"sortKey":null,"ids":["gid://shopify/Product/1","gid://shopify/Product/2"]}"#
        )
    );

    shopify
        .execute(recent_products::Variables {
            first: 5,
            sort_key: Some("CREATED_AT".to_string()),
        })
        .await
        .unwrap();
    assert_eq!(
        mock.graphql_requests().last().unwrap().variables,
        json!({ "first": 5, "sortKey": "CREATED_AT" })
    );
}

#[tokio::test]
async fn one_vec_of_input_objects_goes_to_an_inline_operation_and_to_a_module() {
    let mock = MockTransport::new();
    mock.respond_with(|_| {
        Ok(shopify_api::transport::HttpResponse::json(
            200,
            &json!({ "data": { "metafieldsSet": { "metafields": [], "userErrors": [] } } }),
        ))
    });
    let shopify = client(&mock);
    let metafields = color_metafields(&rows());

    let inline = "mutation($metafields: [MetafieldsSetInput!]!) { metafieldsSet(metafields: $metafields) { userErrors { message } } }";
    shopify
        .execute(graphql!(
            "mutation($metafields: [MetafieldsSetInput!]!) { metafieldsSet(metafields: $metafields) { userErrors { message } } }",
            metafields = &metafields,
        ))
        .await
        .unwrap();
    assert_eq!(
        sent(&mock),
        body(inline, &format!(r#"{{"metafields":{COLOR_METAFIELDS}}}"#))
    );

    let data = shopify
        .execute(metafields_set::Variables { metafields })
        .await
        .unwrap();
    assert!(data.metafields_set.unwrap().user_errors.is_empty());
    assert_eq!(
        sent(&mock),
        format!(
            r#"{{"query":{},"variables":{{"metafields":{COLOR_METAFIELDS}}},"operationName":"MetafieldsSet"}}"#,
            serde_json::to_string(metafields_set::QUERY).unwrap()
        )
    );

    // `operation(variables)` still builds the operation.
    let operation = metafields_set::operation(metafields_set::Variables {
        metafields: color_metafields(&rows()),
    });
    assert_eq!(
        operation.variables_json().unwrap()["metafields"][1]["compareDigest"],
        "d3"
    );
}

#[tokio::test]
async fn input_objects_of_two_call_sites_share_one_vec_and_a_module_type() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "metafieldsSet": { "metafields": [], "userErrors": [] } }));
    let shopify = client(&mock);

    let metafields: Vec<metafields_set::MetafieldsSetInput> = rows()
        .iter()
        .map(|row| match &row.digest {
            Some(digest) => input!(MetafieldsSetInput {
                ownerId: &row.product_id,
                key: "color",
                value: row.value,
                compareDigest: digest,
            }),
            None => input!(MetafieldsSetInput {
                ownerId: &row.product_id,
                key: "color",
                value: row.value,
                type: "single_line_text_field",
            }),
        })
        .collect();
    let first = metafields[0].clone();
    assert_eq!(
        format!("{first:?}"),
        r#"MetafieldsSetInput {"ownerId":"gid://shopify/Product/1","key":"color","value":"blue","type":"single_line_text_field"}"#
    );
    assert_eq!(
        serde_json::to_string(&first).unwrap(),
        r#"{"ownerId":"gid://shopify/Product/1","key":"color","value":"blue","type":"single_line_text_field"}"#
    );

    shopify
        .execute(metafields_set::Variables { metafields })
        .await
        .unwrap();
    assert_eq!(
        mock.graphql_requests()[0].variables,
        json!({ "metafields": [
            { "ownerId": "gid://shopify/Product/1", "key": "color", "value": "blue", "type": "single_line_text_field" },
            { "ownerId": "gid://shopify/Product/2", "key": "color", "value": "", "type": "single_line_text_field" },
            { "ownerId": "gid://shopify/Product/3", "key": "color", "value": "red", "compareDigest": "d3" },
        ] })
    );
}

#[tokio::test]
async fn the_modules_of_one_block_each_run() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "shop": { "name": "Test shop" } }));
    mock.push_data(json!({ "products": { "nodes": [{ "id": "gid://shopify/Product/1" }] } }));
    mock.push_data(json!({ "productsCount": { "count": 3 } }));
    let shopify = client(&mock);

    let data: shop_name::Data = shopify.execute(shop_name::operation()).await.unwrap();
    assert_eq!(data.shop.name, "Test shop");
    let data = shopify
        .execute(recent_products::Variables {
            first: 1,
            sort_key: None,
        })
        .await
        .unwrap();
    assert_eq!(data.products.nodes[0].id, "gid://shopify/Product/1");
    // No variable is required: `Default` cannot leave one out.
    let data = shopify
        .execute(product_count::Variables::default())
        .await
        .unwrap();
    assert_eq!(data.products_count.unwrap().count, 3);

    let names: Vec<Option<String>> = mock
        .graphql_requests()
        .into_iter()
        .map(|request| request.operation_name)
        .collect();
    assert_eq!(
        names,
        [
            Some("ShopName".to_string()),
            Some("RecentProducts".to_string()),
            Some("ProductCount".to_string())
        ]
    );
    assert_eq!(mock.graphql_requests()[1].variables, json!({ "first": 1 }));
}

#[tokio::test]
async fn a_nullable_input_variable_takes_null_an_option_or_a_literal() {
    let mock = MockTransport::new();
    mock.respond_with(|_| {
        Ok(shopify_api::transport::HttpResponse::json(
            200,
            &json!({ "data": { "productCreate": { "product": null } } }),
        ))
    });
    let shopify = client(&mock);
    let query = "mutation($product: ProductCreateInput, $media: [CreateMediaInput!]) { productCreate(product: $product, media: $media) { product { id } } }";

    shopify
        .execute(graphql!(
            "mutation($product: ProductCreateInput, $media: [CreateMediaInput!]) { productCreate(product: $product, media: $media) { product { id } } }",
            product = { title: "Shirt", status: DRAFT },
            media = null,
        ))
        .await
        .unwrap();
    assert_eq!(
        sent(&mock),
        body(
            query,
            r#"{"product":{"title":"Shirt","status":"DRAFT"},"media":null}"#
        )
    );

    let image = input!(CreateMediaInput {
        originalSource: "https://example.com/shirt.jpg",
        mediaContentType: IMAGE,
    });
    let product = Some(input!(ProductCreateInput { title: "Shirt" }));
    shopify
        .execute(graphql!(
            "mutation($product: ProductCreateInput, $media: [CreateMediaInput!]) { productCreate(product: $product, media: $media) { product { id } } }",
            product = product,
            media = [image],
        ))
        .await
        .unwrap();
    assert_eq!(
        sent(&mock),
        body(
            query,
            r#"{"product":{"title":"Shirt"},"media":[{"originalSource":"https://example.com/shirt.jpg","mediaContentType":"IMAGE"}]}"#
        )
    );
}

/// What Shopify would not read as written: an `Int` beyond 32 bits, a
/// `Float` that is not finite, `null` for a non-null `JSON`. The operation
/// fails before anything is sent, saying where and why, never the value.
#[tokio::test]
async fn values_shopify_would_not_read_as_written_fail_before_anything_is_sent() {
    let mock = MockTransport::new();
    let shopify = client(&mock);
    let big: i64 = 1 << 40;
    let quantities = [1_u64, 2, u64::MAX];
    let nan = f64::NAN;
    let weights = [Some(1.5), None, Some(f64::INFINITY)];
    let line_item = input!(DraftOrderLineItemInput { quantity: big });
    let pixel = input!(WebPixelInput {
        settings: serde_json::Value::Null
    });

    let errors = [
        shopify
            .execute(graphql!("mutation($input: DraftOrderInput!) { draftOrderCreate(input: $input) { userErrors { message } } }", input = { lineItems: [{ quantity: 1 }, { quantity: big }] }))
            .await
            .unwrap_err(),
        shopify
            .execute(graphql!("mutation($input: DraftOrderInput!) { draftOrderCreate(input: $input) { userErrors { message } } }", input = {
                lineItems: [{ quantity: 1, weight: { unit: KILOGRAMS, value: nan } }],
            }))
            .await
            .unwrap_err(),
        shopify
            .execute(graphql!("mutation($input: DraftOrderInput!) { draftOrderCreate(input: $input) { userErrors { message } } }", input = {
                lineItems: [for quantity in quantities => { quantity: quantity }],
            }))
            .await
            .unwrap_err(),
        shopify
            .execute(graphql!("mutation($input: DraftOrderInput!) { draftOrderCreate(input: $input) { userErrors { message } } }", input = {
                lineItems: [for (i, weight) in weights.iter().enumerate() => {
                    quantity: i,
                    weight?: weight.map(|value| input!(WeightInput { unit: GRAMS, value: value })),
                }],
            }))
            .await
            .unwrap_err(),
        shopify
            .execute(graphql!("mutation($input: DraftOrderInput!) { draftOrderCreate(input: $input) { userErrors { message } } }", input = { lineItems: [line_item.clone()] }))
            .await
            .unwrap_err(),
        shopify
            .execute(graphql!("mutation($webPixel: WebPixelInput!) { webPixelCreate(webPixel: $webPixel) { userErrors { message } } }", webPixel = pixel.clone()))
            .await
            .unwrap_err(),
        shopify
            .execute(graphql!(
                "mutation($id: ID!, $discount: DiscountAutomaticBasicInput!) { discountAutomaticBasicUpdate(id: $id, automaticBasicDiscount: $discount) { userErrors { message } } }",
                id = "gid://shopify/DiscountAutomaticNode/1",
                discount = { customerGets: { value: { percentage: f64::INFINITY }, items: { all: true } } },
            ))
            .await
            .unwrap_err(),
        shopify
            .execute(recent_products::Variables { first: big, sort_key: None })
            .await
            .unwrap_err(),
        shopify
            .execute(graphql!(
                "query($first: Int!) { products(first: $first) { nodes { id } } }",
                first = u64::MAX,
            ))
            .await
            .unwrap_err(),
    ];
    let shown: Vec<String> = errors.iter().map(ToString::to_string).collect();
    assert_eq!(
        shown,
        [
            "invalid value at `$input.lineItems[1].quantity`: Int out of 32-bit range",
            "invalid value at `$input.lineItems[0].weight.value`: not a finite Float",
            "invalid value at `$input.lineItems[2].quantity`: Int out of 32-bit range",
            "invalid value at `$input.lineItems[2].weight.value`: not a finite Float",
            "invalid value at `$input.lineItems[0].quantity`: Int out of 32-bit range",
            "invalid value at `$webPixel.settings`: null for a non-null JSON field",
            "invalid value at `$discount.customerGets.value.percentage`: not a finite Float",
            "invalid value at `$first`: Int out of 32-bit range",
            "invalid value at `$first`: Int out of 32-bit range",
        ]
    );
    for error in &errors {
        assert!(matches!(
            error,
            shopify_api::Error::InvalidValue { detail: None, .. }
        ));
        let debug = format!("{error:?}");
        assert!(
            !debug.contains("1099511627776") && !debug.contains("NaN"),
            "{debug}"
        );
    }
    assert!(mock.requests().is_empty());

    // Serialized on its own, an input object fails the same way.
    let error = serde_json::to_string(&line_item).unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid value at `DraftOrderLineItemInput.quantity`: Int out of 32-bit range"
    );
    assert_eq!(
        format!("{pixel:?}"),
        "invalid value at `WebPixelInput.settings`: null for a non-null JSON field"
    );
}

/// The bounds are allowed, and nulls inside `JSON`.
#[test]
fn values_at_the_bounds_are_sent() {
    let line_item = input!(DraftOrderLineItemInput {
        quantity: i64::from(i32::MAX),
        weight: { unit: GRAMS, value: f64::MAX },
    });
    let pixel = input!(WebPixelInput {
        settings: json!({ "a": null })
    });
    let discount = input!(DiscountCustomerGetsValueInput { percentage?: Some(f64::MIN_POSITIVE) });
    let constants = input!(DraftOrderLineItemInput {
        quantity: -2_147_483_648,
        title: "t",
        requiresShipping: true,
        weight: { unit: GRAMS, value: 2 },
    });
    assert_eq!(
        serde_json::to_value(&constants).unwrap(),
        json!({ "quantity": i32::MIN, "title": "t", "requiresShipping": true, "weight": { "unit": "GRAMS", "value": 2 } })
    );
    let tags = input!(ProductUpdateInput { tags: ["t"; 40] });
    assert_eq!(
        serde_json::to_value(&tags).unwrap()["tags"],
        json!(vec!["t"; 40])
    );
    assert_eq!(
        serde_json::to_value((&line_item, &pixel, &discount)).unwrap(),
        json!([
            { "quantity": i32::MAX, "weight": { "unit": "GRAMS", "value": f64::MAX } },
            { "settings": { "a": null } },
            { "percentage": f64::MIN_POSITIVE },
        ])
    );
}

/// Names like the ones the macros generate, or generated before, given to
/// the caller's locals, closures, functions, constants and trait methods:
/// the values sent are the caller's.
mod hygiene {
    #![allow(non_upper_case_globals, non_snake_case, dead_code)]

    use serde_json::{Value, json};
    use shopify_api::{graphql, input};

    /// A trait method by value for every type, named like the methods the
    /// generated code calls.
    trait Capture: Sized {
        fn check(self, _: &str) -> &'static str {
            "captured"
        }
        fn optional(self, _: Option<&str>) -> Option<&'static str> {
            Some("captured")
        }
        fn push(self, _: &str) {}
        fn build<A, B>(self, _: A, _: B) -> &'static str {
            "captured"
        }
        fn variable<T, V>(self, _: &str, _: V) -> Self {
            self
        }
        fn __literal_variable<V>(self, _: &str, _: V) -> Self {
            self
        }
    }
    impl<T> Capture for T {}

    fn json(value: impl serde::Serialize) -> Value {
        serde_json::to_value(value).unwrap()
    }

    #[test]
    fn locals_and_closures() {
        let __shopify_api_check1 = |text: &str| format!("closure:{text}");
        let __shopify_api_check3 = |text: &str| format!("closure:{text}");
        let __shopify_api_optional1 = |text: &str| Some(format!("closure:{text}"));
        let __shopify_api_checks = "local";
        let __shopify_api_items = vec!["local"];
        let __shopify_api_variable_0 = "local";
        let check = |text: &str| format!("closure:{text}");
        let Checks = "local";
        let literal = "local";
        let value0 = |text: &str| format!("closure:{text}");
        let V1 = "local";
        let value = input!(ProductUpdateInput {
            title: __shopify_api_check3("a"),
            vendor: __shopify_api_check1("b"),
            handle: check("c"),
            productType: __shopify_api_checks,
            descriptionHtml: __shopify_api_variable_0,
            templateSuffix?: __shopify_api_optional1("d"),
            tags: [for tag in &__shopify_api_items => Checks.to_string() + tag + literal + V1],
            seo: { title: value0("e"), description: literal },
        });
        assert_eq!(
            json(&value),
            json!({
                "title": "closure:a",
                "vendor": "closure:b",
                "handle": "closure:c",
                "productType": "local",
                "descriptionHtml": "local",
                "templateSuffix": "closure:d",
                "tags": ["locallocallocallocal"],
                "seo": { "title": "closure:e", "description": "local" },
            })
        );

        let operation = graphql!(
            "mutation($product: ProductUpdateInput!, $media: [CreateMediaInput!], $first: Int) { productUpdate(product: $product, media: $media) { product { variants(first: $first) { nodes { id } } } userErrors { message } } }",
            product = { title: __shopify_api_check3("e"), tags: __shopify_api_items.clone() },
            media = [for alt in [__shopify_api_variable_0] => { originalSource: check(alt), mediaContentType: IMAGE, alt: alt }],
            first = __shopify_api_items.len(),
        );
        assert_eq!(
            operation.body().unwrap()["variables"],
            json!({
                "product": { "title": "closure:e", "tags": ["local"] },
                "media": [{ "originalSource": "closure:local", "mediaContentType": "IMAGE", "alt": "local" }],
                "first": 1,
            })
        );
    }

    mod items {
        use super::*;

        const __shopify_api_check1: &str = "const";
        const __shopify_api_checks: &str = "const";
        const __shopify_api_items: [&str; 1] = ["const"];
        const __shopify_api_variable_0: &str = "const";
        const Checks: &str = "const";
        const checks: &str = "const";
        const items: &str = "const";
        const literal: &str = "const";
        const value0: &str = "const";
        const V1: &str = "const";
        const value69: &str = "const";
        fn leaf(text: &str) -> String {
            format!("fn:{text}")
        }
        static __shopify_api_optional1: &str = "static";
        fn __shopify_api_check2(text: &str) -> String {
            format!("fn:{text}")
        }
        fn check(text: &str) -> String {
            format!("fn:{text}")
        }
        fn optional(text: Option<&str>) -> Option<String> {
            text.map(|text| format!("fn:{text}"))
        }

        /// More values than the names `literal::names` has.
        #[test]
        fn seventy_values() {
            let value = input!(ProductUpdateInput {
                tags: [
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1,
                    __shopify_api_check1
                ]
            });
            assert_eq!(json(&value)["tags"], json!(vec!["const"; 70]));
        }

        #[test]
        fn functions_constants_and_statics() {
            let value = input!(ProductUpdateInput {
                title: __shopify_api_check2("a"),
                vendor: check("b"),
                handle: __shopify_api_check1,
                productType: __shopify_api_checks,
                descriptionHtml: Checks,
                templateSuffix?: optional(Some(__shopify_api_optional1)),
                tags: [for tag in __shopify_api_items => check(tag)],
                seo: { title: leaf(literal), description: value0.to_string() + V1 },
            });
            assert_eq!(
                json(&value),
                json!({
                    "title": "fn:a",
                    "vendor": "fn:b",
                    "handle": "const",
                    "productType": "const",
                    "descriptionHtml": "const",
                    "templateSuffix": "fn:static",
                    "tags": ["fn:const"],
                    "seo": { "title": "fn:const", "description": "constconst" },
                })
            );
            let operation = graphql!(
                "mutation($product: ProductUpdateInput!, $media: [CreateMediaInput!], $first: Int) { productUpdate(product: $product, media: $media) { product { variants(first: $first) { nodes { id } } } userErrors { message } } }",
                product = { title: __shopify_api_variable_0, tags: [for tag in [checks, items] => tag] },
                first = __shopify_api_items.len(),
            );
            assert_eq!(
                operation.body().unwrap()["variables"],
                json!({ "product": { "title": "const", "tags": ["const", "const"] }, "first": 1 })
            );
        }
    }

    struct Product {
        title: &'static str,
    }

    impl Product {
        const SUFFIX: &'static str = "suffix";

        /// In a method, where `Self` and `self` are the caller's.
        fn input(&self) -> Value {
            json(input!(ProductUpdateInput {
                title: self.title,
                templateSuffix: Self::SUFFIX,
                tags: [for tag in [Self::SUFFIX] => tag],
            }))
        }
    }

    #[test]
    fn self_and_self_type_are_the_callers() {
        assert_eq!(
            Product { title: "t" }.input(),
            json!({ "title": "t", "templateSuffix": "suffix", "tags": ["suffix"] })
        );
    }
}
