//! Tokens: obtained, reused, renewed and saved.
#![cfg(feature = "testing")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use shopify_api::testing::MockTransport;
use shopify_api::transport::{BoxFuture, HttpRequest, HttpResponse};
use shopify_api::{Auth, Error, RetryPolicy, ShopDomain, Shopify, TokenData, TokenStore};

const TOKEN_URL: &str = "https://test-shop.myshopify.com/admin/oauth/access_token";

/// Answers token requests with `shpat_<n>` and GraphQL requests with a shop,
/// refusing the tokens in `refused` with a 401.
fn shopify_like(mock: &MockTransport, expires_in: i64, refused: Vec<&'static str>) {
    let issued = Arc::new(Mutex::new(0));
    mock.respond_with(move |request: &HttpRequest| {
        if request.url == TOKEN_URL {
            let mut issued = issued.lock().unwrap();
            *issued += 1;
            return Ok(HttpResponse::json(
                200,
                &json!({
                    "access_token": format!("shpat_{issued}"),
                    "scope": "read_products",
                    "expires_in": expires_in,
                    "refresh_token": format!("shprt_{issued}"),
                    "refresh_token_expires_in": 7_776_000,
                }),
            ));
        }
        let token = request.header("X-Shopify-Access-Token").unwrap_or_default();
        if refused.contains(&token) {
            return Ok(HttpResponse::new(
                401,
                "[API] Invalid API key or access token",
            ));
        }
        Ok(HttpResponse::json(
            200,
            &json!({ "data": { "shop": { "name": "x" } } }),
        ))
    });
}

fn token_requests(mock: &MockTransport) -> Vec<HttpRequest> {
    mock.requests()
        .into_iter()
        .filter(|request| request.url == TOKEN_URL)
        .collect()
}

fn builder(mock: &MockTransport) -> shopify_api::ShopifyBuilder {
    Shopify::builder("test-shop")
        .transport(mock.clone())
        .retry(RetryPolicy::none())
}

#[tokio::test]
async fn client_credentials_are_asked_once_and_reused() {
    let mock = MockTransport::new();
    shopify_like(&mock, 86_399, vec![]);
    let shopify = builder(&mock)
        .client_credentials("id", "secret")
        .build()
        .unwrap();

    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    shopify
        .clone()
        .graphql("{ shop { name } }", ())
        .await
        .unwrap();

    let asked = token_requests(&mock);
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0].header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        asked[0].form(),
        [
            ("grant_type".to_string(), "client_credentials".to_string()),
            ("client_id".to_string(), "id".to_string()),
            ("client_secret".to_string(), "secret".to_string()),
        ]
    );
    assert_eq!(
        mock.graphql_requests()[1].access_token.as_deref(),
        Some("shpat_1")
    );
    let token = shopify.token().await.unwrap();
    assert_eq!(token.scopes(), ["read_products"]);
    assert!(token.expires_at.is_some());
}

#[tokio::test]
async fn concurrent_requests_share_one_token_request() {
    let mock = MockTransport::new();
    shopify_like(&mock, 86_399, vec![]);
    let shopify = builder(&mock)
        .client_credentials("id", "secret")
        .build()
        .unwrap();

    let calls = (0..8).map(|_| {
        let shopify = shopify.clone();
        tokio::spawn(async move { shopify.graphql("{ shop { name } }", ()).await })
    });
    for call in calls {
        call.await.unwrap().unwrap();
    }
    assert_eq!(token_requests(&mock).len(), 1);
}

#[tokio::test]
async fn a_token_about_to_expire_is_renewed_first() {
    let mock = MockTransport::new();
    // Each token expires within the five minutes of leeway: each request renews it.
    shopify_like(&mock, 60, vec![]);
    let shopify = builder(&mock)
        .client_credentials("id", "secret")
        .build()
        .unwrap();

    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    assert_eq!(token_requests(&mock).len(), 2);

    // With a shorter leeway, the token lasts.
    let mock = MockTransport::new();
    shopify_like(&mock, 60, vec![]);
    let shopify = builder(&mock)
        .client_credentials("id", "secret")
        .refresh_leeway(Duration::from_secs(10))
        .build()
        .unwrap();
    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    assert_eq!(token_requests(&mock).len(), 1);
}

#[tokio::test]
async fn a_refused_token_is_renewed_once_and_the_request_sent_again() {
    let mock = MockTransport::new();
    shopify_like(&mock, 86_399, vec!["stored"]);
    let shopify = builder(&mock)
        .auth(Auth::client_credentials("id", "secret").with_token(TokenData::new("stored")))
        .build()
        .unwrap();

    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    let tokens: Vec<Option<String>> = mock
        .graphql_requests()
        .into_iter()
        .map(|request| request.access_token)
        .collect();
    assert_eq!(
        tokens,
        [Some("stored".to_string()), Some("shpat_1".to_string())]
    );

    // A token refused again is the answer: no loop.
    let mock = MockTransport::new();
    shopify_like(&mock, 86_399, vec!["stored", "shpat_1"]);
    let shopify = builder(&mock)
        .auth(Auth::client_credentials("id", "secret").with_token(TokenData::new("stored")))
        .build()
        .unwrap();
    let error = shopify.graphql("{ shop { name } }", ()).await.unwrap_err();
    assert!(error.is_unauthorized(), "{error:?}");
    assert_eq!(mock.requests().len(), 3);
}

#[tokio::test]
async fn a_static_token_that_is_refused_is_the_answer() {
    let mock = MockTransport::new();
    shopify_like(&mock, 0, vec!["shpat_static"]);
    let shopify = builder(&mock).access_token("shpat_static").build().unwrap();

    let error = shopify.graphql("{ shop { name } }", ()).await.unwrap_err();
    assert!(error.is_unauthorized());
    assert!(token_requests(&mock).is_empty());
}

#[derive(Default, Clone)]
struct Saved(Arc<Mutex<Vec<(String, TokenData)>>>);

impl TokenStore for Saved {
    fn save<'a>(
        &'a self,
        shop: &'a ShopDomain,
        token: &'a TokenData,
    ) -> BoxFuture<'a, Result<(), Box<dyn std::error::Error + Send + Sync>>> {
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
async fn an_expiring_token_is_refreshed_and_saved() {
    let mock = MockTransport::new();
    shopify_like(&mock, 3600, vec![]);
    let saved = Saved::default();
    let mut stored = TokenData::new("shpat_old");
    stored.expires_at = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
    stored.refresh_token = Some("shprt_old".to_string());
    let shopify = builder(&mock)
        .auth(Auth::expiring_token("id", "secret", stored))
        .token_store(saved.clone())
        .build()
        .unwrap();

    shopify.graphql("{ shop { name } }", ()).await.unwrap();

    let refresh = &token_requests(&mock)[0];
    assert_eq!(refresh.header("content-type"), Some("application/json"));
    assert_eq!(
        refresh.json().unwrap(),
        json!({ "grant_type": "refresh_token", "client_id": "id", "client_secret": "secret", "refresh_token": "shprt_old" })
    );
    let saved = saved.0.lock().unwrap();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].0, "test-shop.myshopify.com");
    assert_eq!(saved[0].1.access_token, "shpat_1");
    assert_eq!(saved[0].1.refresh_token.as_deref(), Some("shprt_1"));
    assert_eq!(
        mock.graphql_requests()[0].access_token.as_deref(),
        Some("shpat_1")
    );
}

#[tokio::test]
async fn an_expired_refresh_token_says_what_to_do() {
    let mock = MockTransport::new();
    shopify_like(&mock, 3600, vec![]);
    let mut stored = TokenData::new("shpat_old");
    stored.expires_at = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
    stored.refresh_token = Some("shprt_old".to_string());
    stored.refresh_token_expires_at = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
    let shopify = builder(&mock)
        .auth(Auth::expiring_token("id", "secret", stored))
        .build()
        .unwrap();

    let error = shopify.graphql("{ shop { name } }", ()).await.unwrap_err();
    assert!(matches!(error, Error::Auth(_)), "{error:?}");
    assert!(
        error
            .to_string()
            .contains("refresh token is missing or expired"),
        "{error}"
    );
    assert!(mock.requests().is_empty());
}

#[tokio::test]
async fn a_refused_grant_says_what_shopify_answered_and_never_the_secret() {
    let mock = MockTransport::new();
    mock.push(HttpResponse::json(
        400,
        &json!({ "error": "shop_not_permitted", "error_description": "Client credentials cannot be performed on this shop." }),
    ));
    let shopify = builder(&mock)
        .client_credentials("id", "top-secret")
        .build()
        .unwrap();

    let error = shopify.graphql("{ shop { name } }", ()).await.unwrap_err();
    let message = error.to_string();
    assert_eq!(
        message,
        "authentication failed: the token endpoint of test-shop.myshopify.com answered 400: shop_not_permitted: Client credentials cannot be performed on this shop."
    );
    assert!(!format!("{error:?}").contains("top-secret"));
}

#[tokio::test]
async fn a_token_answer_that_does_not_parse_shows_no_token() {
    for answer in [
        // A token, with a field of the wrong type next to it.
        HttpResponse::json(
            200,
            &json!({ "access_token": "shpat_TOKEN_SECRET", "scope": 1, "refresh_token": "shprt_REFRESH_SECRET", "expires_in": "shpat_TOKEN_SECRET" }),
        ),
        // A refusal that echoes what was sent.
        HttpResponse::new(
            400,
            "invalid request: client_secret=CLIENT_SECRET&refresh_token=shprt_REFRESH_SECRET",
        ),
    ] {
        let mock = MockTransport::new();
        mock.push(answer);
        let shopify = builder(&mock)
            .client_credentials("id", "CLIENT_SECRET")
            .build()
            .unwrap();
        let error = shopify.graphql("{ shop { name } }", ()).await.unwrap_err();
        for shown in [
            format!("{error}"),
            format!("{error:?}"),
            format!("{error:#?}"),
        ] {
            for secret in ["TOKEN_SECRET", "REFRESH_SECRET", "CLIENT_SECRET"] {
                assert!(!shown.contains(secret), "{secret} in {shown}");
            }
            assert!(shown.contains("token endpoint"), "{shown}");
        }
    }
}

#[tokio::test]
async fn a_failed_renewal_ahead_of_expiry_keeps_the_token_that_still_works() {
    let mock = MockTransport::new();
    mock.respond_with(|request: &HttpRequest| {
        if request.url == TOKEN_URL {
            return Ok(HttpResponse::new(503, "unavailable"));
        }
        Ok(HttpResponse::json(
            200,
            &json!({ "data": { "shop": { "name": "x" } } }),
        ))
    });
    // A token that expires in a minute: within the five minutes of leeway.
    let mut stored = TokenData::new("shpat_soon");
    stored.expires_at = Some(chrono::Utc::now() + chrono::Duration::seconds(60));
    let shopify = builder(&mock)
        .auth(Auth::client_credentials("id", "secret").with_token(stored))
        .build()
        .unwrap();

    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    shopify.graphql("{ shop { name } }", ()).await.unwrap();

    // One renewal tried, then held back; the current token sent both times.
    assert_eq!(token_requests(&mock).len(), 1);
    let sent: Vec<_> = mock
        .graphql_requests()
        .into_iter()
        .map(|request| request.access_token)
        .collect();
    assert_eq!(
        sent,
        [
            Some("shpat_soon".to_string()),
            Some("shpat_soon".to_string())
        ]
    );
}

/// Answers token requests after `delay`, and counts them.
struct Slow {
    delay: Duration,
    asked: Arc<Mutex<u32>>,
}

impl shopify_api::transport::Transport for Slow {
    fn send(
        &self,
        request: HttpRequest,
    ) -> BoxFuture<'_, Result<HttpResponse, shopify_api::transport::TransportError>> {
        Box::pin(async move {
            if request.url == TOKEN_URL {
                *self.asked.lock().unwrap() += 1;
                tokio::time::sleep(self.delay).await;
                return Ok(HttpResponse::json(
                    200,
                    &json!({ "access_token": "shpat_new", "expires_in": 3600, "refresh_token": "shprt_new" }),
                ));
            }
            Ok(HttpResponse::json(
                200,
                &json!({ "data": { "shop": { "name": "x" } } }),
            ))
        })
    }
}

#[tokio::test]
async fn a_cancelled_request_does_not_lose_the_rotated_refresh_token() {
    let asked = Arc::new(Mutex::new(0));
    let saved = Saved::default();
    let mut stored = TokenData::new("shpat_old");
    stored.expires_at = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
    stored.refresh_token = Some("shprt_old".to_string());
    let shopify = Shopify::builder("test-shop")
        .auth(Auth::expiring_token("id", "secret", stored))
        .transport(Slow {
            delay: Duration::from_millis(100),
            asked: asked.clone(),
        })
        .token_store(saved.clone())
        .build()
        .unwrap();

    // The caller gives up while Shopify is answering the refresh.
    let gave_up = tokio::time::timeout(Duration::from_millis(10), shopify.access_token()).await;
    assert!(gave_up.is_err());
    tokio::time::sleep(Duration::from_millis(200)).await;

    // The renewal finished anyway: the new tokens are held and saved.
    let token = shopify.token().await.unwrap();
    assert_eq!(token.refresh_token.as_deref(), Some("shprt_new"));
    assert_eq!(saved.0.lock().unwrap().len(), 1);
    assert_eq!(shopify.access_token().await.unwrap(), "shpat_new");
    assert_eq!(*asked.lock().unwrap(), 1);
}

#[tokio::test]
async fn a_failed_grant_answers_the_requests_that_wait_for_it() {
    let mock = MockTransport::new();
    mock.respond_with(|request: &HttpRequest| {
        if request.url == TOKEN_URL {
            return Ok(HttpResponse::new(503, "unavailable"));
        }
        Ok(HttpResponse::json(
            200,
            &json!({ "data": { "shop": { "name": "x" } } }),
        ))
    });
    let shopify = builder(&mock)
        .client_credentials("id", "secret")
        .build()
        .unwrap();

    let calls: Vec<_> = (0..5)
        .map(|_| {
            let shopify = shopify.clone();
            tokio::spawn(async move { shopify.graphql("{ shop { name } }", ()).await })
        })
        .collect();
    for call in calls {
        let error = call.await.unwrap().unwrap_err();
        assert!(matches!(error, Error::Auth(_)), "{error:?}");
    }
    // One request to the token endpoint; the others got its failure.
    assert_eq!(token_requests(&mock).len(), 1);
}

#[tokio::test]
async fn a_token_that_never_expires_in_practice_is_read() {
    let mock = MockTransport::new();
    mock.push_json(json!({ "access_token": "shpat_long", "expires_in": i64::MAX, "refresh_token_expires_in": -1 }));
    mock.push_data(json!({ "shop": { "name": "x" } }));
    let shopify = builder(&mock)
        .client_credentials("id", "secret")
        .build()
        .unwrap();

    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    let token = shopify.token().await.unwrap();
    assert_eq!(
        token.expires_at,
        Some(chrono::DateTime::<chrono::Utc>::MAX_UTC)
    );
    assert!(!token.expires_within(chrono::Duration::days(365)));
}
