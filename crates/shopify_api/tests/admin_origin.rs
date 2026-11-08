//! An admin origin: the requests built from the shop's domain go to a local
//! stand-in of Shopify, over the default transport, and nothing else changes.
#![cfg(all(
    feature = "oauth",
    feature = "testing",
    any(feature = "rustls", feature = "native-tls")
))]

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use shopify_api::oauth::{RequestedToken, ShopifyApp};
use shopify_api::testing::MockTransport;
use shopify_api::{ApiVersion, RetryPolicy, Shopify};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A request the local server received.
#[derive(Debug, Clone)]
struct Received {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Received {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

type Log = Arc<Mutex<Vec<Received>>>;

/// A local stand-in of Shopify's admin on a port of its own: its token
/// endpoint, the GraphQL Admin API and a bulk operation's file. Returns its
/// origin, on `localhost`, and what it receives.
async fn start() -> (String, Log) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log = Log::default();
    let received = log.clone();
    tokio::spawn(async move {
        loop {
            let (socket, _) = listener.accept().await.unwrap();
            tokio::spawn(answer(socket, received.clone()));
        }
    });
    (format!("http://localhost:{port}"), log)
}

async fn answer(mut socket: TcpStream, log: Log) {
    let request = read_request(&mut socket).await;
    let (status, body) = respond(&request);
    log.lock().unwrap().push(request);
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    socket.write_all(head.as_bytes()).await.unwrap();
    socket.write_all(&body).await.unwrap();
    socket.shutdown().await.ok();
}

async fn read_request(socket: &mut TcpStream) -> Received {
    let mut bytes = Vec::new();
    let mut buffer = [0; 4096];
    let end = loop {
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        let read = socket.read(&mut buffer).await.unwrap();
        assert!(read > 0, "the connection closed before the headers ended");
        bytes.extend_from_slice(&buffer[..read]);
    };
    let head = String::from_utf8(bytes[..end].to_vec()).unwrap();
    let mut lines = head.split("\r\n");
    let mut start = lines.next().unwrap().split(' ');
    let method = start.next().unwrap().to_string();
    let path = start.next().unwrap().to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map_or(0, |(_, value)| value.parse().unwrap());
    let mut body = bytes[end + 4..].to_vec();
    while body.len() < length {
        let read = socket.read(&mut buffer).await.unwrap();
        assert!(read > 0, "the connection closed before the body ended");
        body.extend_from_slice(&buffer[..read]);
    }
    Received {
        method,
        path,
        headers,
        body,
    }
}

fn respond(request: &Received) -> (&'static str, Vec<u8>) {
    let json = |value: Value| value.to_string().into_bytes();
    match (request.method.as_str(), request.path.as_str()) {
        ("POST", "/admin/oauth/access_token") => {
            let exchanged = serde_json::from_slice::<Value>(&request.body).is_ok_and(|body| {
                body["grant_type"] == "urn:ietf:params:oauth:grant-type:token-exchange"
            });
            let granted =
                String::from_utf8_lossy(&request.body).contains("grant_type=client_credentials");
            match (exchanged, granted) {
                (true, _) => (
                    "200 OK",
                    json(json!({ "access_token": "shpat_exchanged", "scope": "read_products" })),
                ),
                (_, true) => (
                    "200 OK",
                    json(
                        json!({ "access_token": "shpat_granted", "scope": "read_products", "expires_in": 86_399 }),
                    ),
                ),
                _ => (
                    "400 Bad Request",
                    json(json!({ "error": "unsupported_grant_type" })),
                ),
            }
        }
        ("POST", "/admin/api/2026-10/graphql.json") => {
            match request.header("x-shopify-access-token") {
                Some("shpat_exchanged" | "shpat_granted") => (
                    "200 OK",
                    json(json!({ "data": { "shop": { "name": "Local Supply Co" } } })),
                ),
                _ => (
                    "401 Unauthorized",
                    json(json!({ "errors": "[API] Invalid API key or access token" })),
                ),
            }
        }
        ("GET", "/__lsf/admin/bulk/my-shop.myshopify.com/1.jsonl") => (
            "200 OK",
            b"{\"id\":\"gid://shopify/Product/1\"}\n{\"id\":\"gid://shopify/Product/2\"}\n"
                .to_vec(),
        ),
        _ => ("404 Not Found", json(json!({ "errors": "Not Found" }))),
    }
}

fn session_claims() -> Value {
    let now = chrono::Utc::now().timestamp();
    json!({
        "iss": "https://my-shop.myshopify.com/admin",
        "dest": "https://my-shop.myshopify.com",
        "aud": "client-id",
        "sub": "1",
        "exp": now + 60,
        "nbf": now - 1,
        "iat": now - 1,
        "jti": "a5f0c3e2-7d4b-4f7e-9a51-0c2d6b1e8f30",
        "sid": "local-session",
    })
}

#[tokio::test]
async fn a_token_exchange_and_a_query_go_to_the_admin_origin() {
    let (origin, log) = start().await;
    let app = ShopifyApp::new("client-id", "client-secret")
        .with_admin_origin(&origin)
        .unwrap();

    let bearer = app.sign_session_token(&session_claims());
    let session = app.verify_session_token(&bearer).unwrap();
    let token = app
        .exchange_session_token(&session, RequestedToken::Offline)
        .await
        .unwrap();
    assert_eq!(token.access_token, "shpat_exchanged");

    let shopify = app
        .client(session.shop(), token)
        .unwrap()
        .with_api_version(ApiVersion::V2026_10);
    assert_eq!(shopify.shop().as_str(), "my-shop.myshopify.com");
    let data = shopify.graphql("{ shop { name } }", ()).await.unwrap();
    assert_eq!(data["shop"]["name"], "Local Supply Co");

    let received = log.lock().unwrap().clone();
    assert_eq!(received.len(), 2, "{received:#?}");
    let exchange = &received[0];
    assert_eq!(
        (exchange.method.as_str(), exchange.path.as_str()),
        ("POST", "/admin/oauth/access_token")
    );
    assert_eq!(
        exchange.header("x-forwarded-host"),
        Some("my-shop.myshopify.com")
    );
    let fields: Value = serde_json::from_slice(&exchange.body).unwrap();
    assert_eq!(fields["subject_token"], bearer.as_str());
    assert_eq!(fields["client_id"], "client-id");

    let query = &received[1];
    assert_eq!(
        (query.method.as_str(), query.path.as_str()),
        ("POST", "/admin/api/2026-10/graphql.json")
    );
    assert_eq!(
        query.header("x-shopify-access-token"),
        Some("shpat_exchanged")
    );
    assert_eq!(
        query.header("x-forwarded-host"),
        Some("my-shop.myshopify.com")
    );
}

#[tokio::test]
async fn the_client_credentials_grant_goes_to_the_admin_origin() {
    let (origin, log) = start().await;
    let shopify = Shopify::builder("my-shop")
        .admin_origin(format!("{origin}/"))
        .client_credentials("client-id", "client-secret")
        .api_version(ApiVersion::V2026_10)
        .retry(RetryPolicy::none())
        .build()
        .unwrap();

    let data = shopify.graphql("{ shop { name } }", ()).await.unwrap();
    assert_eq!(data["shop"]["name"], "Local Supply Co");

    let received = log.lock().unwrap().clone();
    let paths: Vec<&str> = received
        .iter()
        .map(|request| request.path.as_str())
        .collect();
    assert_eq!(
        paths,
        [
            "/admin/oauth/access_token",
            "/admin/api/2026-10/graphql.json"
        ]
    );
    assert!(String::from_utf8_lossy(&received[0].body).contains("grant_type=client_credentials"));
    assert!(
        received
            .iter()
            .all(|request| request.header("x-forwarded-host") == Some("my-shop.myshopify.com"))
    );
}

#[tokio::test]
async fn bulk_results_download_from_the_admin_origin() {
    let (origin, log) = start().await;
    let shopify = Shopify::builder("my-shop")
        .admin_origin(&origin)
        .access_token("shpat_exchanged")
        .build()
        .unwrap();

    // The address a bulk operation of local-storefront answers.
    let url = format!("{origin}/__lsf/admin/bulk/my-shop.myshopify.com/1.jsonl");
    let rows: Vec<Value> = shopify.download_bulk_results(&url).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["id"], "gid://shopify/Product/2");

    // As from Shopify's storage: no token, and nothing added.
    let received = log.lock().unwrap().clone();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].header("x-shopify-access-token"), None);
    assert_eq!(received[0].header("x-forwarded-host"), None);
}

#[tokio::test]
async fn without_an_admin_origin_nothing_changes() {
    let mock = MockTransport::new();
    mock.push_json(json!({ "access_token": "shpat_exchanged", "scope": "read_products" }));
    mock.push_data(json!({ "shop": { "name": "x" } }));
    let app = ShopifyApp::new("client-id", "client-secret").with_transport(mock.clone());
    assert_eq!(app.admin_origin(), None);

    let session = app
        .verify_session_token(&app.sign_session_token(&session_claims()))
        .unwrap();
    let token = app
        .exchange_session_token(&session, RequestedToken::Offline)
        .await
        .unwrap();
    let shopify = app
        .client(session.shop(), token)
        .unwrap()
        .with_api_version(ApiVersion::V2026_10);
    assert_eq!(shopify.admin_origin(), None);
    shopify.graphql("{ shop { name } }", ()).await.unwrap();

    let sent = mock.requests();
    assert_eq!(
        sent[0].url,
        "https://my-shop.myshopify.com/admin/oauth/access_token"
    );
    assert_eq!(
        sent[1].url,
        "https://my-shop.myshopify.com/admin/api/2026-10/graphql.json"
    );
    assert!(
        sent.iter()
            .all(|request| request.header("X-Forwarded-Host").is_none())
    );
}

#[tokio::test]
async fn the_admin_origin_goes_through_a_mock_transport_too() {
    let mock = MockTransport::new();
    mock.push_data(json!({ "shop": { "name": "x" } }));
    let shopify = Shopify::builder("my-shop")
        .admin_origin("http://127.0.0.1:9292")
        .access_token("t")
        .api_version(ApiVersion::V2026_10)
        .transport(mock.clone())
        .build()
        .unwrap();
    shopify.graphql("{ shop { name } }", ()).await.unwrap();
    let sent = mock.last_request().unwrap();
    assert_eq!(
        sent.url,
        "http://127.0.0.1:9292/admin/api/2026-10/graphql.json"
    );
    assert_eq!(
        sent.header("X-Forwarded-Host"),
        Some("my-shop.myshopify.com")
    );
}
