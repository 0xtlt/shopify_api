//! Testing code that calls Shopify, without the network.
//!
//! A [`MockTransport`] records the requests a client sends and answers them
//! with what the test scripts: queued answers first, in order, then a
//! handler, then `404`.
//!
//! ```
//! use shopify_api::graphql;
//! use shopify_api::testing::{MockTransport, client};
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> shopify_api::Result<()> {
//! let mock = MockTransport::new();
//! mock.push_data(serde_json::json!({ "shop": { "name": "Test shop" } }));
//! let shopify = client(&mock);
//!
//! let data = shopify.execute(graphql!("query Shop { shop { name } }")).await?;
//!
//! assert_eq!(data.shop.name, "Test shop");
//! let sent = mock.graphql_requests();
//! assert_eq!(sent[0].operation_name.as_deref(), Some("Shop"));
//! # Ok(())
//! # }
//! ```

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde_json::{Value, json};

use crate::transport::{BoxFuture, HttpRequest, HttpResponse, Transport, TransportError};
use crate::{RetryPolicy, Shopify};

type Handler = dyn Fn(&HttpRequest) -> Result<HttpResponse, TransportError> + Send + Sync;

/// A [`Transport`] that answers what the test says. Clones share their state.
#[derive(Clone, Default)]
pub struct MockTransport {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    queue: VecDeque<Result<HttpResponse, TransportError>>,
    handler: Option<Arc<Handler>>,
    requests: Vec<HttpRequest>,
}

impl std::fmt::Debug for MockTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = self.state();
        formatter
            .debug_struct("MockTransport")
            .field("queued", &state.queue.len())
            .field("handler", &state.handler.is_some())
            .field("requests", &state.requests.len())
            .finish()
    }
}

impl MockTransport {
    pub fn new() -> Self {
        Self::default()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Answers every request that finds no queued answer.
    pub fn respond_with(
        &self,
        handler: impl Fn(&HttpRequest) -> Result<HttpResponse, TransportError> + Send + Sync + 'static,
    ) -> &Self {
        self.state().handler = Some(Arc::new(handler));
        self
    }

    /// Answers the next request that finds no answer queued before this one.
    pub fn push(&self, response: HttpResponse) -> &Self {
        self.state().queue.push_back(Ok(response));
        self
    }

    /// Fails the next request, as a network failure.
    pub fn push_error(&self, error: TransportError) -> &Self {
        self.state().queue.push_back(Err(error));
        self
    }

    /// Answers the next request with `200` and `body`.
    pub fn push_json(&self, body: Value) -> &Self {
        self.push(HttpResponse::json(200, &body))
    }

    /// Answers the next request with `200` and `{ "data": data }`.
    pub fn push_data(&self, data: Value) -> &Self {
        self.push_json(json!({ "data": data }))
    }

    /// Answers the next request with `200` and Shopify's `THROTTLED` error.
    pub fn push_throttled(&self) -> &Self {
        self.push_json(json!({
            "errors": [{ "message": "Throttled", "extensions": { "code": "THROTTLED" } }],
            "extensions": { "cost": {
                "requestedQueryCost": 10,
                "actualQueryCost": null,
                "throttleStatus": { "maximumAvailable": 2000.0, "currentlyAvailable": 0, "restoreRate": 100.0 },
            } },
        }))
    }

    /// Every request sent so far, in order.
    pub fn requests(&self) -> Vec<HttpRequest> {
        self.state().requests.clone()
    }

    pub fn last_request(&self) -> Option<HttpRequest> {
        self.state().requests.last().cloned()
    }

    /// The GraphQL requests sent so far, read.
    pub fn graphql_requests(&self) -> Vec<GraphqlRequest> {
        self.state()
            .requests
            .iter()
            .filter_map(GraphqlRequest::read)
            .collect()
    }

    /// Forgets the requests, the queued answers and the handler.
    pub fn clear(&self) {
        *self.state() = State::default();
    }
}

impl Transport for MockTransport {
    fn send(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, TransportError>> {
        let answer = {
            let mut state = self.state();
            state.requests.push(request.clone());
            match state.queue.pop_front() {
                Some(answer) => answer,
                None => match state.handler.clone() {
                    Some(handler) => {
                        drop(state);
                        handler(&request)
                    }
                    None => Ok(HttpResponse::new(
                        404,
                        format!("not mocked: {:?} {}", request.method, request.url),
                    )),
                },
            }
        };
        Box::pin(async move { answer })
    }
}

/// A GraphQL request, as a [`MockTransport`] received it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct GraphqlRequest {
    pub url: String,
    /// The version of the address: `2026-10`.
    pub api_version: String,
    pub query: String,
    /// The variables, `{}` when there are none.
    pub variables: Value,
    pub operation_name: Option<String>,
    pub access_token: Option<String>,
}

impl GraphqlRequest {
    fn read(request: &HttpRequest) -> Option<Self> {
        let (_, path) = request.url.split_once("/admin/api/")?;
        let (api_version, rest) = path.split_once('/')?;
        if rest != "graphql.json" {
            return None;
        }
        let body = request.json()?;
        Some(Self {
            url: request.url.clone(),
            api_version: api_version.to_string(),
            query: body.get("query")?.as_str()?.to_string(),
            variables: body.get("variables").cloned().unwrap_or_else(|| json!({})),
            operation_name: body
                .get("operationName")
                .and_then(Value::as_str)
                .map(str::to_string),
            access_token: request.header("X-Shopify-Access-Token").map(str::to_string),
        })
    }
}

/// A client of `test-shop.myshopify.com` with the token `test-token`,
/// sending through `mock`, retrying without waiting.
pub fn client(mock: &MockTransport) -> Shopify {
    Shopify::builder("test-shop")
        .access_token("test-token")
        .transport(mock.clone())
        .retry(
            RetryPolicy::new(4)
                .with_base_delay(Duration::ZERO)
                .with_max_delay(Duration::ZERO),
        )
        .build()
        .expect("a valid test client")
}
