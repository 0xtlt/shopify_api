//! Webhook subscriptions against a scripted Shopify.
#![cfg(all(feature = "testing", feature = "webhooks"))]

use serde_json::{Value, json};
use shopify_api::testing::{MockTransport, client};
use shopify_api::webhooks::WebhookSubscriptionSpec;

fn subscription(id: u32, topic: &str, uri: &str) -> Value {
    json!({
        "id": format!("gid://shopify/WebhookSubscription/{id}"),
        "topic": topic,
        "uri": uri,
        "format": "JSON",
        "filter": null,
        "includeFields": [],
        "metafieldNamespaces": [],
        "name": null,
        "createdAt": "2026-10-01T00:00:00Z",
        "updatedAt": "2026-10-01T00:00:00Z",
        "apiVersion": { "handle": "2026-10" },
    })
}

#[tokio::test]
async fn sync_keeps_updates_creates_then_deletes() {
    let mock = MockTransport::new();
    // Two pages of existing subscriptions.
    mock.push_data(json!({ "webhookSubscriptions": {
        "nodes": [subscription(1, "ORDERS_CREATE", "https://example.com/hooks")],
        "pageInfo": { "hasNextPage": true, "endCursor": "c1" },
    } }));
    mock.push_data(json!({ "webhookSubscriptions": {
        "nodes": [
            subscription(2, "PRODUCTS_UPDATE", "https://old.example.com/hooks"),
            subscription(3, "PRODUCTS_UPDATE", "https://example.com/hooks"),
        ],
        "pageInfo": { "hasNextPage": false, "endCursor": "c2" },
    } }));
    let mut updated = subscription(3, "PRODUCTS_UPDATE", "https://example.com/hooks");
    updated["includeFields"] = json!(["id"]);
    mock.push_data(json!({ "webhookSubscriptionUpdate": { "webhookSubscription": updated, "userErrors": [] } }));
    mock.push_data(json!({ "webhookSubscriptionCreate": {
        "webhookSubscription": subscription(4, "APP_UNINSTALLED", "https://example.com/hooks"),
        "userErrors": [],
    } }));
    mock.push_data(json!({ "webhookSubscriptionDelete": {
        "deletedWebhookSubscriptionId": "gid://shopify/WebhookSubscription/2",
        "userErrors": [],
    } }));
    let shopify = client(&mock);

    let report = shopify
        .sync_webhook_subscriptions(&[
            WebhookSubscriptionSpec::new("orders/create", "https://example.com/hooks"),
            WebhookSubscriptionSpec::new("products/update", "https://example.com/hooks")
                .include_fields(["id"]),
            WebhookSubscriptionSpec::new("app/uninstalled", "https://example.com/hooks"),
        ])
        .await
        .unwrap();

    assert_eq!(report.kept.len(), 1);
    assert_eq!(report.updated[0].include_fields, ["id"]);
    assert_eq!(report.created[0].topic, "APP_UNINSTALLED");
    assert_eq!(report.created[0].api_version, "2026-10");
    assert_eq!(report.deleted, ["gid://shopify/WebhookSubscription/2"]);

    let sent = mock.graphql_requests();
    let names: Vec<_> = sent
        .iter()
        .map(|request| request.operation_name.clone().unwrap())
        .collect();
    // Nothing is deleted before what replaces it exists.
    assert_eq!(
        names,
        [
            "WebhookSubscriptions",
            "WebhookSubscriptions",
            "WebhookSubscriptionUpdate",
            "WebhookSubscriptionCreate",
            "WebhookSubscriptionDelete",
        ]
    );
    assert_eq!(sent[1].variables, json!({ "after": "c1" }));
    // An update sends every option, so that it replaces the old ones.
    assert_eq!(
        sent[2].variables,
        json!({
            "id": "gid://shopify/WebhookSubscription/3",
            "webhookSubscription": {
                "uri": "https://example.com/hooks",
                "format": "JSON",
                "filter": "",
                "includeFields": ["id"],
                "metafieldNamespaces": [],
            },
        })
    );
    assert_eq!(
        sent[3].variables,
        json!({
            "topic": "APP_UNINSTALLED",
            "webhookSubscription": { "uri": "https://example.com/hooks", "format": "JSON" },
        })
    );
}

#[tokio::test]
async fn a_refused_subscription_returns_the_user_errors() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "webhookSubscriptionCreate": {
        "webhookSubscription": null,
        "userErrors": [{ "field": ["webhookSubscription", "callbackUrl"], "message": "Address is invalid" }],
    } }));
    let shopify = client(&mock);

    let error = shopify
        .create_webhook_subscription(&WebhookSubscriptionSpec::new(
            "ORDERS_CREATE",
            "http://localhost",
        ))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "webhookSubscriptionCreate refused: webhookSubscription.callbackUrl: Address is invalid"
    );
}
