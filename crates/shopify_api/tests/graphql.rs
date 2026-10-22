//! `graphql!` operations end to end: what is sent, and how the answer is typed.
#![cfg(feature = "testing")]

use serde_json::{Value, json};
use shopify_api::testing::{MockTransport, client};
use shopify_api::{ApiVersion, Error, OperationKind, graphql};

graphql! {
    /// Writes metafields and reports what Shopify refused.
    pub mod metafields_set = "#graphql
        mutation MetafieldsSet($metafields: [MetafieldsSetInput!]!) {
          metafieldsSet(metafields: $metafields) {
            metafields { id key namespace }
            userErrors { field message code }
          }
        }";
}

graphql! {
    mod shop_name = "query ShopName { shop { name } }";
}

graphql! {
    mod from_file = file("tests/graphql/order.graphql");
}

#[tokio::test]
async fn an_inline_query_sends_its_variables_and_returns_typed_data() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "orders": { "nodes": [
        { "id": "gid://shopify/Order/1", "name": "#1001" },
        { "id": "gid://shopify/Order/2", "name": "#1002" },
    ] } }));
    let shopify = client(&mock);
    let limit: u32 = 2;

    let data = shopify
        .execute(graphql!(
            "query Recent($first: Int!, $query: String) { orders(first: $first, query: $query) { nodes { id name } } }",
            first = limit,
            query = "status:open",
        ))
        .await
        .unwrap();

    let names: Vec<&str> = data
        .orders
        .nodes
        .iter()
        .map(|order| order.name.as_str())
        .collect();
    assert_eq!(names, ["#1001", "#1002"]);

    let sent = &mock.graphql_requests()[0];
    assert_eq!(
        sent.url,
        "https://test-shop.myshopify.com/admin/api/2026-10/graphql.json"
    );
    assert_eq!(sent.api_version, "2026-10");
    assert_eq!(
        sent.variables,
        json!({ "first": 2, "query": "status:open" })
    );
    assert_eq!(sent.operation_name.as_deref(), Some("Recent"));
    assert_eq!(sent.access_token.as_deref(), Some("test-token"));
    let request = mock.last_request().unwrap();
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert!(
        request
            .header("user-agent")
            .unwrap()
            .starts_with("shopify_api/")
    );
}

#[tokio::test]
async fn nullability_lists_aliases_fragments_and_scalars_follow_the_schema() {
    let mock = MockTransport::new();
    mock.push_data(json!({
        "order": {
            "id": "gid://shopify/Order/1",
            "note": null,
            "createdAt": "2026-10-01T00:00:00Z",
            "tags": ["vip", "b2b"],
            "displayFinancialStatus": "PAID",
            "total": { "shopMoney": { "amount": "42.50", "currencyCode": "EUR" } },
            "customer": null,
            "metafield": { "jsonValue": { "gift": true }, "type": "json" },
            "lineItems": { "nodes": [{ "title": "Shirt", "quantity": 2 }] },
        },
        "nodes": [
            { "__typename": "Product", "id": "gid://shopify/Product/1", "title": "Shirt" },
            null,
            { "__typename": "Collection", "id": "gid://shopify/Collection/1" },
        ],
    }));
    let shopify = client(&mock);

    let data = shopify
        .execute(graphql!(
            r#"#graphql
            query Order($id: ID!, $ids: [ID!]!, $namespace: String) {
              order(id: $id) {
                id
                note
                createdAt
                tags
                displayFinancialStatus
                total: totalPriceSet { shopMoney { amount currencyCode } }
                customer { displayName }
                metafield(namespace: $namespace, key: "settings") { jsonValue type }
                lineItems(first: 5) { nodes { title quantity } }
              }
              nodes(ids: $ids) {
                __typename
                id
                ... on Product { title }
              }
            }"#,
            id = "gid://shopify/Order/1",
            ids = ["gid://shopify/Product/1", "gid://shopify/Collection/1"],
            namespace = "custom",
        ))
        .await
        .unwrap();

    // `order` is nullable in the schema.
    let order = data.order.expect("an order");
    let note: Option<String> = order.note;
    assert_eq!(note, None);
    let created_at: String = order.created_at;
    assert_eq!(created_at, "2026-10-01T00:00:00Z");
    assert_eq!(order.tags, ["vip", "b2b"]);
    // Enum values are strings.
    assert_eq!(order.display_financial_status.as_deref(), Some("PAID"));
    // An alias names its field.
    assert_eq!(order.total.shop_money.amount, "42.50");
    assert_eq!(order.customer, None);
    // `JSON` is any value.
    let metafield = order.metafield.unwrap();
    assert_eq!(metafield.json_value, json!({ "gift": true }));
    // `Int` is `i64`.
    let quantity: i64 = order.line_items.nodes[0].quantity;
    assert_eq!(quantity, 2);

    // A field under a fragment on another type is an `Option`.
    let nodes = data.nodes;
    assert_eq!(nodes.len(), 3);
    let product = nodes[0].as_ref().unwrap();
    assert_eq!(
        (product.typename.as_str(), product.title.as_deref()),
        ("Product", Some("Shirt"))
    );
    assert!(nodes[1].is_none());
    assert_eq!(nodes[2].as_ref().unwrap().title, None);
}

#[tokio::test]
async fn a_module_names_its_types_and_builds_input_objects() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "metafieldsSet": {
        "metafields": [{ "id": "gid://shopify/Metafield/1", "key": "color", "namespace": "custom" }],
        "userErrors": [],
    } }));
    let shopify = client(&mock);

    let variables = metafields_set::Variables {
        metafields: vec![metafields_set::MetafieldsSetInput {
            owner_id: "gid://shopify/Product/1".to_string(),
            namespace: Some("custom".to_string()),
            key: "color".to_string(),
            value: "blue".to_string(),
            ..Default::default()
        }],
    };
    let data: metafields_set::Data = shopify
        .execute(metafields_set::operation(variables))
        .await
        .unwrap();
    let payload: metafields_set::DataMetafieldsSet = data.metafields_set.unwrap();
    assert!(payload.user_errors.is_empty());
    assert_eq!(payload.metafields.unwrap()[0].key, "color");

    assert_eq!(metafields_set::NAME, Some("MetafieldsSet"));
    assert_eq!(metafields_set::KIND, OperationKind::Mutation);
    assert_eq!(metafields_set::API_VERSION, ApiVersion::DEFAULT.to_string());
    // Fields left to `None` are not sent.
    assert_eq!(
        mock.graphql_requests()[0].variables,
        json!({ "metafields": [{ "ownerId": "gid://shopify/Product/1", "namespace": "custom", "key": "color", "value": "blue" }] })
    );
}

#[tokio::test]
async fn an_operation_can_come_from_a_file() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "order": { "name": "#1001", "email": null } }));
    let shopify = client(&mock);

    let data = shopify
        .execute(from_file::operation(from_file::Variables {
            id: "gid://shopify/Order/1".to_string(),
        }))
        .await
        .unwrap();
    assert_eq!(data.order.unwrap().name, "#1001");
    assert_eq!(from_file::QUERY, include_str!("graphql/order.graphql"));
    assert_eq!(
        mock.graphql_requests()[0].operation_name.as_deref(),
        Some("OrderFromFile")
    );
}

#[tokio::test]
async fn graphql_errors_fail_the_operation_and_keep_partial_data() {
    let mock = MockTransport::new();
    mock.push_json(json!({
        "data": { "shop": null },
        "errors": [{ "message": "Access denied for shop field.", "extensions": { "code": "ACCESS_DENIED" }, "path": ["shop"] }],
    }));
    let shopify = client(&mock);

    let error = shopify.execute(shop_name::operation()).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "GraphQL errors: Access denied for shop field. (ACCESS_DENIED)"
    );
    let Error::Graphql(errors) = &error else {
        panic!("GraphQL errors: {error:?}");
    };
    assert_eq!(errors.data, Some(json!({ "shop": null })));
    assert_eq!(error.graphql_errors()[0].code(), Some("ACCESS_DENIED"));

    // The raw answer holds both.
    mock.push_json(json!({ "data": { "shop": null }, "errors": [{ "message": "Denied" }] }));
    let raw = shopify.graphql_raw("{ shop { name } }", ()).await.unwrap();
    assert_eq!(raw.errors[0].message, "Denied");
    assert_eq!(raw.data, Some(json!({ "shop": null })));
}

graphql! {
    /// One key, `price`, on fields of two types below exclusive object types.
    mod prices = "query Prices($ids: [ID!]!) {
        nodes(ids: $ids) {
          ... on Product { price: priceRangeV2 { value: minVariantPrice { amount currencyCode } } }
          ... on Order { price: totalPriceSet { value: shopMoney { amount currencyCode } } }
        }
      }";
}

#[tokio::test]
async fn one_key_on_fields_of_different_types_reads_both() {
    let mock = MockTransport::new();
    let money = json!({ "value": { "amount": "10.0", "currencyCode": "EUR" } });
    mock.push_data(json!({ "nodes": [
        { "price": money },
        { "price": { "value": { "amount": "25.5", "currencyCode": "USD" } } },
        {},
        null,
    ] }));
    let shopify = client(&mock);

    let data = shopify
        .execute(prices::operation(prices::Variables {
            ids: vec![
                "gid://shopify/Product/1".into(),
                "gid://shopify/Order/2".into(),
            ],
        }))
        .await
        .unwrap();
    let price = |index: usize| {
        let node: &Option<prices::DataNodes> = &data.nodes[index];
        node.as_ref()
            .and_then(|node| node.price.as_ref())
            .map(|price| {
                let prices::DataNodesPrice {
                    value:
                        prices::DataNodesPriceValue {
                            amount,
                            currency_code,
                        },
                } = price;
                (amount.as_str(), currency_code.as_str())
            })
    };
    assert_eq!(price(0), Some(("10.0", "EUR")));
    assert_eq!(price(1), Some(("25.5", "USD")));
    assert_eq!(price(2), None);
    assert!(data.nodes[3].is_none());
}

#[tokio::test]
async fn an_error_shows_no_value_shopify_echoes_in_its_extensions() {
    // What GraphQL Ruby answers to a variable that does not validate: the
    // whole value, a signed address included, under `extensions.value`.
    let mock = MockTransport::new();
    mock.push_json(json!({ "errors": [{
        "message": "Variable $media of type [CreateMediaInput!] was provided invalid value for 0.mediaContentType (Expected \"INVALID\" to be one of: IMAGE, VIDEO)",
        "locations": [{ "line": 1, "column": 10 }],
        "extensions": {
            "value": [{
                "originalSource": "https://storage.test/image.jpg?X-Goog-Signature=SIGNED_SECRET",
                "mediaContentType": "INVALID",
            }],
            "problems": [{ "path": [0, "mediaContentType"], "explanation": "Expected \"INVALID\" to be one of: IMAGE, VIDEO" }],
            "code": "INVALID_VARIABLE",
        },
    }] }));
    let shopify = client(&mock);

    let error = shopify
        .graphql(
            "mutation($media: [CreateMediaInput!]) { productCreate(media: $media) { product { id } } }",
            json!({ "media": [{
                "originalSource": "https://storage.test/image.jpg?X-Goog-Signature=SIGNED_SECRET",
                "mediaContentType": "INVALID",
            }] }),
        )
        .await
        .unwrap_err();
    for shown in [
        format!("{error}"),
        format!("{error:?}"),
        format!("{error:#?}"),
        format!("{:?}", error.graphql_errors()),
    ] {
        assert!(!shown.contains("SIGNED_SECRET"), "{shown}");
        assert!(!shown.contains("storage.test"), "{shown}");
    }
    let debug = format!("{error:?}");
    assert!(debug.contains(r#""code": "INVALID_VARIABLE""#), "{debug}");
    assert!(debug.contains(r#""value": .."#), "{debug}");
    assert!(debug.contains(r#""problems": .."#), "{debug}");
    assert!(debug.contains("Variable $media"), "{debug}");
    // The values stay available to the caller.
    let extensions = error.graphql_errors()[0].extensions.as_ref().unwrap();
    assert!(extensions["value"][0]["originalSource"].is_string());
}

#[tokio::test]
async fn an_answer_that_does_not_fit_names_the_path() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "shop": {} }));
    let shopify = client(&mock);

    let error = shopify.execute(shop_name::operation()).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Shopify's answer does not match the operation at `shop`: missing field `name`"
    );
}

#[tokio::test]
async fn http_errors_say_what_shopify_answered() {
    let mock = MockTransport::new();
    mock.push(shopify_api::transport::HttpResponse::new(
        403,
        "[API] This action requires merchant approval for write_orders scope.",
    ));
    let shopify = client(&mock);

    let error = shopify.execute(shop_name::operation()).await.unwrap_err();
    assert_eq!(error.status(), Some(403));
    assert_eq!(
        error.to_string(),
        "Shopify answered 403 to https://test-shop.myshopify.com/admin/api/2026-10/graphql.json: [API] This action requires merchant approval for write_orders scope."
    );
}

#[tokio::test]
async fn untyped_calls_take_any_serializable_variables() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "product": { "title": "Shirt" } }));
    mock.push_data(json!({ "product": { "title": "Shirt" } }));
    let shopify = client(&mock);

    let data: Value = shopify
        .graphql(
            "query($id: ID!) { product(id: $id) { title } }",
            json!({ "id": "gid://shopify/Product/1" }),
        )
        .await
        .unwrap();
    assert_eq!(data.pointer("/product/title"), Some(&json!("Shirt")));

    #[derive(serde::Deserialize)]
    struct Data {
        product: Option<Product>,
    }
    #[derive(serde::Deserialize)]
    struct Product {
        title: String,
    }
    #[derive(serde::Serialize)]
    struct Variables<'a> {
        id: &'a str,
    }
    let data: Data = shopify
        .graphql_as(
            "query($id: ID!) { product(id: $id) { title } }",
            Variables {
                id: "gid://shopify/Product/1",
            },
        )
        .await
        .unwrap();
    assert_eq!(data.product.unwrap().title, "Shirt");

    let error = shopify
        .graphql("{ shop { name } }", [1, 2])
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid variables: the variables must serialize to an object"
    );
    // Nothing was sent for it.
    assert_eq!(mock.requests().len(), 2);
}

#[tokio::test]
async fn operations_go_to_the_version_of_their_schema_unless_one_is_pinned() {
    let mock = MockTransport::new();
    mock.respond_with(|_| {
        Ok(shopify_api::transport::HttpResponse::json(
            200,
            &json!({ "data": { "shop": { "name": "x" } } }),
        ))
    });
    let shopify = client(&mock);

    shopify.execute(shop_name::operation()).await.unwrap();
    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    let pinned = shopify.with_api_version(ApiVersion::V2026_07);
    pinned.execute(shop_name::operation()).await.unwrap();
    pinned.graphql("{ shop { name } }", ()).await.unwrap();

    let versions: Vec<String> = mock
        .graphql_requests()
        .into_iter()
        .map(|request| request.api_version)
        .collect();
    assert_eq!(versions, ["2026-10", "2026-10", "2026-07", "2026-07"]);
}

#[tokio::test]
async fn the_response_carries_the_cost() {
    let mock = MockTransport::new();
    mock.push_json(json!({
        "data": { "shop": { "name": "x" } },
        "extensions": { "cost": {
            "requestedQueryCost": 1,
            "actualQueryCost": 1,
            "throttleStatus": { "maximumAvailable": 2000.0, "currentlyAvailable": 1999, "restoreRate": 100.0 },
        } },
    }));
    let shopify = client(&mock);

    let response = shopify
        .execute_response(shop_name::operation())
        .await
        .unwrap();
    assert_eq!(response.data.shop.name, "x");
    let cost = response.cost().unwrap();
    assert_eq!(cost.actual_query_cost, Some(1.0));
    assert_eq!(cost.throttle_status.currently_available, 1999.0);
}
