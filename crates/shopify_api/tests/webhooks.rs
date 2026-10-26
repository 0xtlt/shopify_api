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

/// Only JSON webhooks verify: an XML subscription of a desired topic and
/// address is updated to JSON, not kept.
#[tokio::test]
async fn sync_updates_an_xml_subscription_to_json() {
    let mock = MockTransport::new();
    let mut xml = subscription(1, "ORDERS_CREATE", "https://example.com/hooks");
    xml["format"] = json!("XML");
    mock.push_data(json!({ "webhookSubscriptions": {
        "nodes": [xml],
        "pageInfo": { "hasNextPage": false, "endCursor": null },
    } }));
    mock.push_data(json!({ "webhookSubscriptionUpdate": {
        "webhookSubscription": subscription(1, "ORDERS_CREATE", "https://example.com/hooks"),
        "userErrors": [],
    } }));
    let shopify = client(&mock);

    let report = shopify
        .sync_webhook_subscriptions(&[WebhookSubscriptionSpec::new(
            "orders/create",
            "https://example.com/hooks",
        )])
        .await
        .unwrap();

    assert!(report.kept.is_empty());
    assert_eq!(report.updated[0].format, "JSON");
    let sent = mock.graphql_requests();
    assert_eq!(
        sent[1].operation_name.as_deref(),
        Some("WebhookSubscriptionUpdate")
    );
    assert_eq!(sent[1].variables["webhookSubscription"]["format"], "JSON");
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

mod verification {
    use shopify_api::webhooks::{Webhook, sign, verify_hmac};

    const SECRET: &str = "hush";

    fn headers(signature: &str, topic: &str, shop: &str) -> Vec<(String, String)> {
        vec![
            ("X-Shopify-Hmac-Sha256".into(), signature.into()),
            ("X-Shopify-Topic".into(), topic.into()),
            ("X-Shopify-Shop-Domain".into(), shop.into()),
        ]
    }

    fn refusal(signature: &str, body: &[u8]) -> String {
        let result = Webhook::verify(
            headers(signature, "shop/redact", "victim.myshopify.com"),
            body,
            SECRET,
        );
        assert!(!verify_hmac(SECRET, body, signature));
        result.unwrap_err().to_string()
    }

    /// The other texts the client secret signs, which an installer of the
    /// app reads, never pass for a webhook.
    #[cfg(feature = "oauth")]
    mod forgeries {
        use base64::Engine;
        use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
        use serde_json::json;
        use shopify_api::oauth::ShopifyApp;

        use super::*;

        const NOT_A_WEBHOOK: &str = "verification failed: the body of the webhook is not a JSON object: only JSON webhooks verify";

        #[test]
        fn the_hmac_of_a_launch_query_is_not_a_webhook_signature() {
            let app = ShopifyApp::new("client-id", SECRET);
            // Sorted, nothing to escape: the message signed is the query itself.
            let query = "host=YWRtaW4&shop=attacker.myshopify.com&timestamp=1700000000";
            let hmac = app.sign_query(query).unwrap();
            let signature = STANDARD.encode(hex::decode(hmac).unwrap());
            assert_eq!(refusal(&signature, query.as_bytes()), NOT_A_WEBHOOK);
        }

        #[test]
        fn the_signature_of_a_session_token_is_not_a_webhook_signature() {
            let app = ShopifyApp::new("client-id", SECRET);
            let token = app.sign_session_token(&json!({
                "dest": "https://attacker.myshopify.com",
                "aud": "client-id",
                "exp": 1,
            }));
            let (signed, signature) = token.rsplit_once('.').unwrap();
            let signature = STANDARD.encode(URL_SAFE_NO_PAD.decode(signature).unwrap());
            assert_eq!(refusal(&signature, signed.as_bytes()), NOT_A_WEBHOOK);
        }

        /// The parameters of an app proxy request are the storefront's: a
        /// name that sorts first can open a JSON array, never an object.
        #[test]
        fn an_app_proxy_request_is_not_a_webhook() {
            let app = ShopifyApp::new("client-id", SECRET);
            let query = "%5B%22=&logged_in_customer_id=&path_prefix=%2Fapps%2Fx\
                         &shop=attacker.myshopify.com&timestamp=1700000000&z=%22%5D";
            let message = "[\"=logged_in_customer_id=path_prefix=/apps/xshop=attacker.myshopify.comtimestamp=1700000000z=\"]";
            assert!(serde_json::from_str::<serde_json::Value>(message).is_ok());
            let signature = app.sign_app_proxy(query).unwrap();
            let signature = STANDARD.encode(hex::decode(signature).unwrap());
            assert_eq!(
                sign(SECRET, message.as_bytes()),
                signature,
                "the message signed"
            );
            assert_eq!(refusal(&signature, message.as_bytes()), NOT_A_WEBHOOK);
        }

        /// `<` sorts before letters: a parameter named `<?xml version` comes
        /// first in a signed query, which then reads as an XML document.
        #[test]
        fn a_signed_query_that_reads_as_xml_is_not_a_webhook() {
            let app = ShopifyApp::new("client-id", SECRET);
            let query = "%3C%3Fxml%20version=%221.0%22%3F%3E%3Corder%20a%3D%22\
                         &path_prefix=%2Fapps%2Fx&shop=attacker.myshopify.com\
                         &timestamp=1700000000&z=%22%2F%3E";
            let message = "<?xml version=\"1.0\"?><order a=\"path_prefix=/apps/xshop=attacker.myshopify.comtimestamp=1700000000z=\"/>";
            let signature = app.sign_app_proxy(query).unwrap();
            let signature = STANDARD.encode(hex::decode(signature).unwrap());
            assert_eq!(
                sign(SECRET, message.as_bytes()),
                signature,
                "the message signed"
            );
            assert_eq!(refusal(&signature, message.as_bytes()), NOT_A_WEBHOOK);

            // The same text as the sorted `key=value` pairs of a launch query.
            let launch =
                "<?xml version=\"1.0\"?><order/>=&shop=attacker.myshopify.com&timestamp=1700000000";
            assert_eq!(
                refusal(&sign(SECRET, launch.as_bytes()), launch.as_bytes()),
                NOT_A_WEBHOOK
            );
        }
    }

    #[test]
    fn only_json_objects_verify() {
        for body in [&b"{\"id\":1}"[..], b"\n  {}"] {
            assert!(
                Webhook::verify(
                    headers(&sign(SECRET, body), "orders/create", "a"),
                    body,
                    SECRET
                )
                .is_ok()
            );
            assert!(verify_hmac(SECRET, body, &sign(SECRET, body)));
        }
        for body in [
            &b""[..],
            b"  ",
            b"[1]",
            b"a=1",
            b"eyJhbGciOiJIUzI1NiJ9.e30",
            b"<?xml version=\"1.0\"?><order/>",
            b" <order/>",
        ] {
            refusal(&sign(SECRET, body), body);
        }
    }

    #[test]
    fn headers_without_x_and_in_any_case_are_read() {
        let body = br#"{"id":1}"#;
        let signature = sign(SECRET, body);
        let headers = [
            ("shopify-hmac-sha256", signature.as_str()),
            ("Shopify-Topic", "orders/create"),
            ("SHOPIFY-SHOP-DOMAIN", "a.myshopify.com"),
            ("shopify-api-version", "2026-10"),
            ("shopify-webhook-id", "w1"),
        ];
        let webhook = Webhook::verify(headers, &body[..], SECRET).unwrap();
        assert_eq!(webhook.topic(), "orders/create");
        assert_eq!(webhook.shop().as_str(), "a.myshopify.com");
        assert_eq!(webhook.api_version(), Some("2026-10"));
        assert_eq!(webhook.webhook_id(), Some("w1"));
    }

    /// axum's and actix's `HeaderMap` is `http`'s, which reqwest re-exports.
    #[cfg(feature = "rustls")]
    #[test]
    fn a_header_map_goes_in_as_it_is() {
        use shopify_api::transport::reqwest_0_13::reqwest::header::{HeaderMap, HeaderValue};

        let body = br#"{"id":1}"#;
        let mut map = HeaderMap::new();
        map.insert(
            "x-shopify-hmac-sha256",
            HeaderValue::from_str(&sign(SECRET, body)).unwrap(),
        );
        map.insert("x-shopify-topic", HeaderValue::from_static("orders/create"));
        map.insert(
            "x-shopify-shop-domain",
            HeaderValue::from_static("a.myshopify.com"),
        );
        // A value that is not UTF-8 is not Shopify's: ignored.
        map.insert("x-other", HeaderValue::from_bytes(&[0xff]).unwrap());
        let webhook = Webhook::verify(&map, body.to_vec(), SECRET).unwrap();
        assert_eq!(webhook.shop().as_str(), "a.myshopify.com");
    }

    #[test]
    fn debug_shows_the_length_of_the_body_not_the_body() {
        let body = br#"{"email":"ada@example.com"}"#;
        let webhook = Webhook::verify(
            headers(&sign(SECRET, body), "customers/create", "a"),
            &body[..],
            SECRET,
        )
        .unwrap();
        let shown = format!("{webhook:?}");
        assert!(!shown.contains("ada@example.com"), "{shown}");
        assert!(shown.contains("[27 bytes]"), "{shown}");
    }

    #[test]
    fn a_destructive_webhook_checks_the_shop_of_its_payload() {
        let verified = |body: &str, shop: &str| {
            Webhook::verify(
                headers(&sign(SECRET, body.as_bytes()), "shop/redact", shop),
                body.as_bytes(),
                SECRET,
            )
            .unwrap()
            .verify_payload_shop()
        };
        let redact = r#"{"shop_id":1,"shop_domain":"a.myshopify.com"}"#;
        assert!(verified(redact, "a.myshopify.com").is_ok());
        assert_eq!(
            verified(redact, "victim.myshopify.com")
                .unwrap_err()
                .to_string(),
            "verification failed: the payload of the webhook does not name the shop of its headers"
        );
        let uninstalled =
            r#"{"id":1,"domain":"shop.example.com","myshopify_domain":"a.myshopify.com"}"#;
        assert!(verified(uninstalled, "a").is_ok());
        assert!(verified(r#"{"id":1}"#, "a").is_err());
        assert!(verified("{not json", "a").is_err());
    }
}
