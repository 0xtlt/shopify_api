use shopify_api::{Shopify, ShopifyAuth, ShopifyConfig};

fn assert_send<T: Send>(_: T) {}

#[test]
fn admin_graphql_futures_remain_send() {
    let shopify = Shopify::new(
        "example",
        ShopifyAuth::AccessToken("token".to_string()),
        ShopifyConfig::default(),
    )
    .unwrap();
    let variables = serde_json::json!({});

    assert_send(shopify.graphql_raw("query { shop { id } }", &variables));
    assert_send(shopify.graphql::<serde_json::Value, _>("query { shop { id } }", &variables));
    assert_send(shopify.graphql_at_path::<serde_json::Value, _>(
        "query { shop { id } }",
        &variables,
        &[],
    ));
}
