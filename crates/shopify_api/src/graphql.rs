//! Running GraphQL operations: typed ones from [`graphql!`](crate::graphql!),
//! untyped ones from any text.

mod operation;
mod response;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

pub use operation::{IntoVariable, Operation, OperationKind};
pub use response::{
    Cost, GraphqlError, Location, RawResponse, Response, ThrottleStatus, UserError,
};

use crate::error::GraphqlErrors;
use crate::transport::{HttpRequest, Method};
use crate::{ApiVersion, Error, Result, Shopify};

pub(crate) use response::decode;

impl Shopify {
    /// Runs an operation and returns its typed `data`.
    ///
    /// Fails when Shopify answers with `errors` (after retrying a throttled
    /// request), and when `data` does not fit the operation. The `userErrors`
    /// of a mutation are part of `data`: read them.
    ///
    /// ```no_run
    /// # async fn run(shopify: shopify_api::Shopify) -> shopify_api::Result<()> {
    /// let data = shopify
    ///     .execute(shopify_api::graphql!(
    ///         "query($first: Int!) { products(first: $first) { nodes { id title } } }",
    ///         first = 10,
    ///     ))
    ///     .await?;
    /// for product in data.products.nodes {
    ///     println!("{} {}", product.id, product.title);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn execute<D: DeserializeOwned>(&self, operation: Operation<D>) -> Result<D> {
        Ok(self.execute_response(operation).await?.data)
    }

    /// [`Shopify::execute`], with the `extensions` of the answer: the cost of
    /// the operation and the state of the rate limit.
    pub async fn execute_response<D: DeserializeOwned>(
        &self,
        operation: Operation<D>,
    ) -> Result<Response<D>> {
        let version = self.version_for(operation.checked_against())?;
        let body = operation.body()?;
        let raw = self.send_graphql(version, &body, operation.kind()).await?;
        let (data, extensions) = into_data(raw)?;
        Ok(Response {
            data: decode(data)?,
            extensions,
        })
    }

    /// Runs any query or mutation and returns its `data`. Nothing checks the
    /// text: prefer [`graphql!`](crate::graphql!) for an operation known
    /// when the crate compiles.
    ///
    /// `variables` is anything that serializes to an object, or `()` for none.
    ///
    /// ```no_run
    /// # async fn run(shopify: shopify_api::Shopify) -> shopify_api::Result<()> {
    /// let data = shopify
    ///     .graphql("query($id: ID!) { product(id: $id) { title } }", serde_json::json!({ "id": "gid://shopify/Product/1" }))
    ///     .await?;
    /// let title = data.pointer("/product/title").and_then(|title| title.as_str());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn graphql(&self, query: &str, variables: impl Serialize) -> Result<Value> {
        let raw = self.graphql_raw(query, variables).await?;
        Ok(into_data(raw)?.0)
    }

    /// [`Shopify::graphql`], with `data` read as `T`.
    pub async fn graphql_as<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: impl Serialize,
    ) -> Result<T> {
        decode(self.graphql(query, variables).await?)
    }

    /// Runs any query or mutation and returns the answer as Shopify sent
    /// it, `errors` included: for an operation whose partial `data` matters.
    /// A throttled request is still retried.
    pub async fn graphql_raw(&self, query: &str, variables: impl Serialize) -> Result<RawResponse> {
        let variables = match serde_json::to_value(variables)
            .map_err(|error| Error::Variables(error.to_string()))?
        {
            Value::Null => Map::new(),
            Value::Object(variables) => variables,
            _ => {
                return Err(Error::Variables(
                    "the variables must serialize to an object".to_string(),
                ));
            }
        };
        let mut body = Map::new();
        body.insert("query".to_string(), Value::String(query.to_string()));
        if !variables.is_empty() {
            body.insert("variables".to_string(), Value::Object(variables));
        }
        let version = self.version_for(None)?;
        self.send_graphql(version, &Value::Object(body), OperationKind::of(query))
            .await
    }

    /// Sends a GraphQL body to `version`, retrying what Shopify did not run,
    /// and renewing the token once when Shopify refuses it.
    pub(crate) async fn send_graphql(
        &self,
        version: ApiVersion,
        body: &Value,
        kind: OperationKind,
    ) -> Result<RawResponse> {
        let url = self.graphql_url(version);
        let body = serde_json::to_vec(body)?;
        let endpoint = self.endpoint();
        let mut token = self.shared.tokens.get(endpoint).await?;
        let mut retries = 0;
        let mut renewed = false;
        loop {
            let request = HttpRequest::new(Method::Post, &url)
                .with_header("Content-Type", "application/json")
                .with_header("Accept", "application/json")
                .with_header("User-Agent", &self.shared.endpoint.user_agent)
                .with_header("X-Shopify-Access-Token", &token)
                .with_body(body.clone());
            let may_retry = retries < self.retry.max_retries();
            let response = match self.shared.endpoint.transport.send(request).await {
                Ok(response) => response,
                Err(error) if may_retry && !kind.is_mutation() => {
                    tracing::debug!("retrying a query after a network failure: {error}");
                    tokio::time::sleep(self.retry.backoff(retries)).await;
                    retries += 1;
                    continue;
                }
                Err(source) => return Err(Error::transport(&url, source)),
            };

            match response.status {
                200..=299 => {}
                401 if !renewed => {
                    renewed = true;
                    match self.shared.tokens.renew(endpoint, &token).await? {
                        Some(new) => {
                            token = new;
                            continue;
                        }
                        None => return Err(Error::http(&url, &response)),
                    }
                }
                429 if may_retry => {
                    let wait = retry_after(&response)
                        .unwrap_or_else(|| self.retry.backoff(retries))
                        .min(self.retry.max_delay());
                    tracing::debug!("Shopify answered 429: retrying in {wait:?}");
                    tokio::time::sleep(wait).await;
                    retries += 1;
                    continue;
                }
                429 => {
                    return Err(Error::Throttled {
                        retries,
                        cost: None,
                    });
                }
                500..=599 if may_retry && !kind.is_mutation() => {
                    tracing::debug!("Shopify answered {}: retrying the query", response.status);
                    tokio::time::sleep(self.retry.backoff(retries)).await;
                    retries += 1;
                    continue;
                }
                _ => return Err(Error::http(&url, &response)),
            }

            let raw: RawResponse = serde_json::from_slice(&response.body)?;
            if raw.is_throttled() {
                let cost = raw.cost();
                if !may_retry {
                    return Err(Error::Throttled { retries, cost });
                }
                let wait = self.retry.throttled(retries, cost);
                tracing::debug!("throttled by Shopify: retrying in {wait:?}");
                tokio::time::sleep(wait).await;
                retries += 1;
                continue;
            }
            return Ok(raw);
        }
    }
}

/// The wait a `Retry-After` header asks for, in seconds; `None` for a
/// value that is not a wait.
fn retry_after(response: &crate::transport::HttpResponse) -> Option<std::time::Duration> {
    let seconds: f64 = response.header("Retry-After")?.trim().parse().ok()?;
    std::time::Duration::try_from_secs_f64(seconds).ok()
}

/// The `data` and `extensions` of an answer without errors.
fn into_data(raw: RawResponse) -> Result<(Value, Option<Value>)> {
    if !raw.errors.is_empty() {
        return Err(Error::Graphql(GraphqlErrors {
            errors: raw.errors,
            data: raw.data,
        }));
    }
    Ok((raw.data.unwrap_or(Value::Null), raw.extensions))
}
