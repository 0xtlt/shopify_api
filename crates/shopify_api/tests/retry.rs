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
    mock.push(HttpResponse::new(429, "Too Many Requests").with_header("Retry-After", "-5"))
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
    assert_eq!(mock.requests().len(), 3);

    // Too long to wait for: the caller gets it.
    mock.clear();
    mock.push(HttpResponse::new(429, "Too Many Requests").with_header("Retry-After", "1e300"));
    let error = shopify.graphql("{ shop { name } }", ()).await.unwrap_err();
    assert!(
        matches!(
            error,
            Error::Throttled {
                retry_after: Some(Duration::MAX),
                ..
            }
        ),
        "{error:?}"
    );
}

/// `max_delay` caps the backoff the client computes, not the wait Shopify asks for.
#[tokio::test(start_paused = true)]
async fn a_429_waits_for_a_retry_after_longer_than_the_longest_backoff() {
    let mock = MockTransport::new();
    mock.push(HttpResponse::new(429, "Too Many Requests").with_header("Retry-After", "120"))
        .push_data(shop());
    let shopify = Shopify::builder("test-shop")
        .access_token("t")
        .transport(mock.clone())
        .build()
        .unwrap();

    let started = tokio::time::Instant::now();
    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    assert_eq!(started.elapsed(), Duration::from_secs(120));
    assert_eq!(mock.requests().len(), 2);
}

/// A wait longer than `max_retry_after` is not slept through: the caller gets it.
#[tokio::test(start_paused = true)]
async fn a_retry_after_beyond_the_limit_is_returned_as_a_throttle_error() {
    let mock = MockTransport::new();
    mock.push(HttpResponse::new(429, "Too Many Requests").with_header("Retry-After", "600"));
    let shopify = Shopify::builder("test-shop")
        .access_token("t")
        .transport(mock.clone())
        .retry(RetryPolicy::new(4).with_max_retry_after(Duration::from_secs(300)))
        .build()
        .unwrap();

    let started = tokio::time::Instant::now();
    let error = shopify.graphql("{ shop { name } }", ()).await.unwrap_err();
    assert_eq!(started.elapsed(), Duration::ZERO);
    let Error::Throttled {
        retries,
        retry_after,
        ..
    } = error
    else {
        panic!("throttled: {error:?}");
    };
    assert_eq!(retries, 0);
    assert_eq!(retry_after, Some(Duration::from_secs(600)));
    assert_eq!(mock.requests().len(), 1);
}

/// A comment that ends with a bare `\r` still hides nothing: the mutation
/// after it is sent once.
#[tokio::test]
async fn a_mutation_after_a_comment_ended_by_a_carriage_return_is_not_sent_again() {
    let mock = MockTransport::new();
    mock.push(HttpResponse::new(502, "Bad Gateway"))
        .push_data(json!({ "tagsAdd": { "userErrors": [] } }));
    let shopify = client(&mock);

    let mutation = "# comment\rmutation { tagsAdd(id: \"gid://shopify/Order/1\", tags: [\"a\"]) { userErrors { message } } }";
    let error = shopify.graphql(mutation, ()).await.unwrap_err();
    assert_eq!(error.status(), Some(502));
    assert_eq!(mock.requests().len(), 1);
}

/// The test client retries a `429` at once, whatever its `Retry-After`; one
/// beyond the limit still ends in an error.
#[tokio::test(start_paused = true)]
async fn the_test_client_does_not_wait_for_a_retry_after() {
    let mock = MockTransport::new();
    mock.push(HttpResponse::new(429, "Too Many Requests").with_header("Retry-After", "300"))
        .push_data(shop());
    let shopify = client(&mock);

    let started = tokio::time::Instant::now();
    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    assert_eq!(started.elapsed(), Duration::ZERO);
    assert_eq!(mock.requests().len(), 2);

    mock.clear();
    mock.push(HttpResponse::new(429, "Too Many Requests").with_header("Retry-After", "301"));
    let error = shopify.graphql("{ shop { name } }", ()).await.unwrap_err();
    assert!(
        matches!(error, Error::Throttled { retry_after: Some(wait), .. } if wait == Duration::from_secs(301)),
        "{error:?}"
    );
    assert_eq!(started.elapsed(), Duration::ZERO);
}
