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
        "iss": "https://my-shop.myshopify.com/admin",
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

#[test]
fn a_checkout_extension_session_token_verifies_on_its_own_path() {
    let app = ShopifyApp::new("client-id", "client-secret");
    let now = now();
    // As a checkout UI extension sends it: `dest` without `https://`, no `iss`, a customer.
    let bearer = app.sign_session_token(&json!({
        "dest": "store-name.myshopify.com",
        "aud": "client-id",
        "exp": now + 60,
        "nbf": now,
        "iat": now,
        "jti": "6c992878-dbaf-48d1-bb9d-6d9b59814fd1",
        "sub": "gid://shopify/Customer/1",
    }));
    let session = app.verify_extension_session_token(&bearer).unwrap();
    assert_eq!(session.shop().as_str(), "store-name.myshopify.com");
    assert_eq!(session.customer(), Some("gid://shopify/Customer/1"));
    assert!(app.verify_session_token(&bearer).is_err());

    // A bare handle, or another host, is still not a shop.
    for dest in ["store-name", "store-name.example.com", "https://store-name"] {
        let bearer =
            app.sign_session_token(&json!({ "dest": dest, "aud": "client-id", "exp": now + 60 }));
        assert!(
            app.verify_extension_session_token(&bearer).is_err(),
            "{dest}"
        );
    }
}

/// The reproduction of the review of 1.0: the token any buyer gets from
/// `shopify.sessionToken.get()` in a checkout UI extension (no `iss`, no `sid`,
/// no `sub`, `dest` without `https://`) must not pass as an admin session,
/// which an app trusts to load the stored offline token of `shop()`.
#[test]
fn a_buyer_checkout_token_is_not_an_admin_session() {
    let app = ShopifyApp::new("client-id", "client-secret");
    let now = now();
    let checkout = app.sign_session_token(&json!({
        "dest": "victim.myshopify.com", "aud": "client-id", "exp": now + 300, "nbf": now - 1,
        "iat": now, "jti": "x"
    }));
    assert_eq!(
        app.verify_session_token(&checkout).unwrap_err().to_string(),
        "verification failed: the `dest` of the session token is not the admin of a shop"
    );
    let extension = app.verify_extension_session_token(&checkout).unwrap();
    assert_eq!(extension.shop().as_str(), "victim.myshopify.com");
    assert_eq!(extension.customer(), None);

    // With `https://` added to `dest`, it still has no `iss`.
    let checkout = app.sign_session_token(&json!({
        "dest": "https://victim.myshopify.com", "aud": "client-id", "exp": now + 300,
    }));
    assert!(app.verify_session_token(&checkout).is_err());

    // The admin token of the same shop verifies on both paths.
    let admin = app.sign_session_token(&json!({
        "iss": "https://victim.myshopify.com/admin", "dest": "https://victim.myshopify.com",
        "aud": "client-id", "sub": "42", "sid": "s", "exp": now + 60, "nbf": now - 1
    }));
    let session = app.verify_session_token(&admin).unwrap();
    assert_eq!(session.shop().as_str(), "victim.myshopify.com");
    assert_eq!(session.user_id(), Some("42"));
    assert_eq!(session.session_id(), Some("s"));
}

/// `iss` was compared as a prefix: a host that starts with the shop's passed.
#[test]
fn an_issuer_that_only_starts_with_the_shop_is_refused() {
    let app = ShopifyApp::new("client-id", "client-secret");
    let now = now();
    let token = app.sign_session_token(&json!({
        "iss": "https://s.myshopify.com.evil.example/admin", "dest": "https://s.myshopify.com",
        "aud": "client-id", "exp": now + 60,
    }));
    assert!(app.verify_session_token(&token).is_err());
    assert!(app.verify_extension_session_token(&token).is_err());
}

#[derive(Default, Clone)]
struct Saved(std::sync::Arc<std::sync::Mutex<Vec<(String, shopify_api::TokenData)>>>);

impl shopify_api::TokenStore for Saved {
    fn save<'a>(
        &'a self,
        shop: &'a shopify_api::ShopDomain,
        token: &'a shopify_api::TokenData,
    ) -> shopify_api::transport::BoxFuture<'a, Result<(), Box<dyn std::error::Error + Send + Sync>>>
    {
        Box::pin(async move {
            self.0
                .lock()
                .unwrap()
                .push((shop.to_string(), token.clone()));
            Ok(())
        })
    }
}

#[tokio::test]
async fn the_clients_of_an_app_save_their_renewed_tokens() {
    let mock = MockTransport::new();
    mock.push_json(
        json!({ "access_token": "shpat_2", "expires_in": 3600, "refresh_token": "shprt_2" }),
    );
    mock.push_data(json!({ "shop": { "name": "My shop" } }));
    let saved = Saved::default();
    let app = app(&mock).with_token_store(saved.clone());

    let mut token = shopify_api::TokenData::new("shpat_1");
    token.expires_at = Some(chrono::Utc::now());
    token.refresh_token = Some("shprt_1".to_string());
    let shop = shopify_api::ShopDomain::new("my-shop").unwrap();
    let shopify = app.client(&shop, token).unwrap();
    shopify.graphql("{ shop { name } }", ()).await.unwrap();

    let saved = saved.0.lock().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].0, "my-shop.myshopify.com");
    assert_eq!(saved[0].1.refresh_token.as_deref(), Some("shprt_2"));
}
