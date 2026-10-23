//! The token requests of OAuth and of the token exchange.
#![cfg(all(feature = "testing", feature = "oauth"))]

use serde_json::json;
use shopify_api::oauth::{RequestedToken, ShopifyApp};
use shopify_api::testing::MockTransport;
use shopify_api::transport::HttpResponse;

fn app(mock: &MockTransport) -> ShopifyApp {
    ShopifyApp::new("client-id", "client-secret").with_transport(mock.clone())
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[tokio::test]
async fn the_code_of_a_verified_callback_becomes_a_token() {
    let mock = MockTransport::new();
    mock.push_json(json!({ "access_token": "shpat_1", "scope": "read_products,write_orders" }));
    let app = app(&mock);

    let query = format!(
        "code=abc&shop=my-shop.myshopify.com&state=s1&timestamp={}",
        now()
    );
    let signature = app.sign_query(&query).unwrap();
    let callback = app
        .verify_query(&format!("{query}&hmac={signature}"))
        .unwrap();
    let token = app.exchange_code(&callback).await.unwrap();

    assert_eq!(token.access_token, "shpat_1");
    assert_eq!(token.scopes(), ["read_products", "write_orders"]);
    let request = mock.last_request().unwrap();
    assert_eq!(
        request.url,
        "https://my-shop.myshopify.com/admin/oauth/access_token"
    );
    assert_eq!(
        request.json().unwrap(),
        json!({ "client_id": "client-id", "client_secret": "client-secret", "code": "abc" })
    );
}

#[tokio::test]
async fn a_session_token_becomes_an_expiring_token_and_a_client_that_refreshes_it() {
    let mock = MockTransport::new();
    mock.push_json(json!({
        "access_token": "shpat_1",
        "scope": "read_products",
        "expires_in": 3600,
        "refresh_token": "shprt_1",
        "refresh_token_expires_in": 7_776_000,
    }));
    let app = app(&mock);
    let now = now();
    let bearer = app.sign_session_token(&json!({
        "iss": "https://my-shop.myshopify.com/admin",
        "dest": "https://my-shop.myshopify.com",
        "aud": "client-id",
        "sub": "42",
        "exp": now + 60,
        "nbf": now,
    }));

    let session = app.verify_session_token(&bearer).unwrap();
    let token = app
        .exchange_session_token(&session, RequestedToken::ExpiringOffline)
        .await
        .unwrap();
    assert_eq!(token.refresh_token.as_deref(), Some("shprt_1"));
    assert_eq!(
        mock.last_request().unwrap().json().unwrap(),
        json!({
            "client_id": "client-id",
            "client_secret": "client-secret",
            "grant_type": "urn:ietf:params:oauth:grant-type:token-exchange",
            "subject_token": bearer,
            "subject_token_type": "urn:ietf:params:oauth:token-type:id_token",
            "requested_token_type": "urn:shopify:params:oauth:token-type:offline-access-token",
            "expiring": "1",
        })
    );

    // The client of that token sends it, through the app's transport.
    mock.push_data(json!({ "shop": { "name": "My shop" } }));
    let shopify = app.client(session.shop(), token).unwrap();
    let data = shopify.graphql("{ shop { name } }", ()).await.unwrap();
    assert_eq!(data["shop"]["name"], "My shop");
    assert_eq!(
        mock.graphql_requests()[0].access_token.as_deref(),
        Some("shpat_1")
    );
}

#[tokio::test]
async fn an_online_token_names_its_staff_member() {
    let mock = MockTransport::new();
    mock.push(HttpResponse::json(
        200,
        &json!({
            "access_token": "shpua_1",
            "scope": "read_products",
            "expires_in": 86_399,
            "associated_user_scope": "read_products",
            "associated_user": { "id": 902541635, "first_name": "Ada", "email": "ada@example.com", "account_owner": true },
        }),
    ));
    let app = app(&mock);
    let now = now();
    let bearer = app.sign_session_token(&json!({
        "dest": "https://my-shop.myshopify.com",
        "aud": "client-id",
        "exp": now + 60,
    }));
    let session = app.verify_session_token(&bearer).unwrap();

    let token = app
        .exchange_session_token(&session, RequestedToken::Online)
        .await
        .unwrap();
    let user = token.associated_user.unwrap();
    assert_eq!(user.id, 902541635);
    assert_eq!(user.first_name.as_deref(), Some("Ada"));
    assert_eq!(user.account_owner, Some(true));
}
