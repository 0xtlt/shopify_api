//! What the client sends again, and what it does not.
#![cfg(feature = "testing")]

use std::time::Duration;

use serde_json::json;
use shopify_api::testing::{MockTransport, client};
use shopify_api::transport::{HttpResponse, TransportError};
use shopify_api::{Error, RetryPolicy, Shopify};

fn shop() -> serde_json::Value {
    json!({ "shop": { "name": "x" } })
}

#[tokio::test]
async fn a_throttled_request_is_sent_again() {
    let mock = MockTransport::new();
    mock.push_throttled().push_throttled().push_data(shop());
    let shopify = client(&mock);

    let data = shopify
        .execute(shopify_api::graphql!("{ shop { name } }"))
        .await
        .unwrap();
    assert_eq!(data.shop.name, "x");
    assert_eq!(mock.requests().len(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_throttled_request_waits_for_its_cost() {
    let mock = MockTransport::new();
    // 10 requested, 0 available, 100 per second: 0.1 s, and a little more.
    mock.push_throttled().push_data(shop());
    let shopify = Shopify::builder("test-shop")
        .access_token("t")
        .transport(mock.clone())
        .build()
        .unwrap();

    let started = tokio::time::Instant::now();
    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    assert_eq!(started.elapsed(), Duration::from_millis(200));
}

#[tokio::test]
async fn throttling_that_lasts_ends_in_an_error_with_the_cost() {
    let mock = MockTransport::new();
    for _ in 0..5 {
        mock.push_throttled();
    }
    let shopify = client(&mock);

    let error = shopify.graphql("{ shop { name } }", ()).await.unwrap_err();
    let Error::Throttled { retries, cost, .. } = error else {
        panic!("throttled: {error:?}");
    };
    assert_eq!(retries, 4);
    assert_eq!(cost.unwrap().requested_query_cost, 10.0);
    assert_eq!(mock.requests().len(), 5);
}

#[tokio::test(start_paused = true)]
async fn a_429_waits_for_its_retry_after() {
    let mock = MockTransport::new();
    mock.push(HttpResponse::new(429, "Too Many Requests").with_header("Retry-After", "2.0"))
        .push_data(shop());
    let shopify = Shopify::builder("test-shop")
        .access_token("t")
        .transport(mock.clone())
        .build()
        .unwrap();

    let started = tokio::time::Instant::now();
    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    assert_eq!(started.elapsed(), Duration::from_secs(2));
}

#[tokio::test]
async fn a_query_is_sent_again_after_a_server_error_or_a_network_failure() {
    let mock = MockTransport::new();
    mock.push(HttpResponse::new(502, "Bad Gateway"))
        .push_error(TransportError::timeout("timed out"))
        .push_data(shop());
    let shopify = client(&mock);

    shopify
        .graphql("query { shop { name } }", ())
        .await
        .unwrap();
    assert_eq!(mock.requests().len(), 3);
}

#[tokio::test]
async fn a_mutation_is_not_sent_again_when_it_may_have_run() {
    let mock = MockTransport::new();
    mock.push(HttpResponse::new(502, "Bad Gateway"));
    let shopify = client(&mock);

    let mutation = "mutation { tagsAdd(id: \"gid://shopify/Order/1\", tags: [\"a\"]) { userErrors { message } } }";
    let error = shopify.graphql(mutation, ()).await.unwrap_err();
    assert_eq!(error.status(), Some(502));
    assert_eq!(mock.requests().len(), 1);

    mock.clear();
    mock.push_error(TransportError::new("connection reset"));
    let error = shopify.graphql(mutation, ()).await.unwrap_err();
    assert!(matches!(error, Error::Transport { .. }), "{error:?}");
    assert_eq!(mock.requests().len(), 1);

    // A throttled mutation did not run: it goes again.
    mock.clear();
    mock.push_throttled()
        .push_data(json!({ "tagsAdd": { "userErrors": [] } }));
    shopify.graphql(mutation, ()).await.unwrap();
    assert_eq!(mock.requests().len(), 2);
}

#[tokio::test]
async fn no_retry_policy_sends_once() {
    let mock = MockTransport::new();
    mock.push_throttled();
    let shopify = client(&mock).with_retry(RetryPolicy::none());

    let error = shopify.graphql("{ shop { name } }", ()).await.unwrap_err();
    assert!(
        matches!(error, Error::Throttled { retries: 0, .. }),
        "{error:?}"
    );
    assert_eq!(mock.requests().len(), 1);
}

#[tokio::test]
async fn waits_shopify_cannot_mean_do_not_panic() {
    let mock = MockTransport::new();
    mock.push(HttpResponse::new(429, "Too Many Requests").with_header("Retry-After", "1e300"))
        .push(HttpResponse::new(429, "Too Many Requests").with_header("Retry-After", "-5"))
        .push_json(json!({
            "errors": [{ "message": "Throttled", "extensions": { "code": "THROTTLED" } }],
            "extensions": { "cost": {
                "requestedQueryCost": 10,
                "throttleStatus": { "maximumAvailable": 2000.0, "currentlyAvailable": 0, "restoreRate": 1e-320 },
            } },
        }))
        .push_data(shop());
    let shopify = client(&mock);

    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    assert_eq!(mock.requests().len(), 4);
}
