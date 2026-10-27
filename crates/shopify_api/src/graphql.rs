//! Running GraphQL operations: typed ones from [`graphql!`](crate::graphql!),
//! untyped ones from any text.

pub(crate) mod check;
mod input;
pub mod literal;
pub(crate) mod operation;
mod response;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

pub use check::{InvalidValueReason, VariableType};
pub use input::{InputObject, Maybe};
pub use operation::{IntoVariable, Operation, OperationKind};
pub use response::{
    Cost, GraphqlError, Location, RawResponse, Response, ThrottleStatus, UserError,
};

use crate::error::GraphqlErrors;
use crate::transport::{HttpRequest, Method};
use crate::{ApiVersion, Error, Result, Shopify};

impl Shopify {
    /// Runs an operation and returns its typed `data`: an [`Operation`], or
    /// the `Variables` of a [`graphql!`](crate::graphql!) module.
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
    pub async fn execute<D: DeserializeOwned>(
        &self,
        operation: impl Into<Operation<D>>,
    ) -> Result<D> {
        Ok(self.execute_response(operation).await?.data)
    }

    /// [`Shopify::execute`], with the `extensions` of the answer: the cost of
    /// the operation and the state of the rate limit.
    pub async fn execute_response<D: DeserializeOwned>(
        &self,
        operation: impl Into<Operation<D>>,
    ) -> Result<Response<D>> {
        let operation = operation.into();
        let version = if operation.is_own() {
            // Written for the version of the crate's schema, and kept working
            // on the versions it supports: a pinned one is not a mistake.
            self.pinned_api_version()
                .map_or_else(|| self.version_for(operation.checked_against()), Ok)?
        } else {
            self.version_for(operation.checked_against())?
        };
        let body = operation.body()?.into_bytes();
        let raw = self.send_graphql(version, body, operation.kind()).await?;
        let (data, extensions) = into_data(raw)?;
        Ok(Response {
            data: crate::decode::from_value(data)?,
            extensions,
        })
    }

    /// [`Shopify::execute`] for one of this crate's operations.
    pub(crate) async fn execute_own<D: DeserializeOwned>(
        &self,
        operation: impl Into<Operation<D>>,
    ) -> Result<D> {
        self.execute(operation.into().own()).await
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
        crate::decode::from_value(self.graphql(query, variables).await?)
    }

    /// Runs any query or mutation and returns the answer as Shopify sent
    /// it, `errors` included: for an operation whose partial `data` matters.
    /// A throttled request is still retried.
    pub async fn graphql_raw(&self, query: &str, variables: impl Serialize) -> Result<RawResponse> {
        let operation = Operation::<Value>::new(query.to_string()).variables(variables);
        let body = operation.body()?.into_bytes();
        self.send_graphql(self.version_for(None)?, body, operation.kind())
            .await
    }

    /// Sends a GraphQL body to `version`, retrying what Shopify did not run,
    /// and renewing the token once when Shopify refuses it.
    pub(crate) async fn send_graphql(
        &self,
        version: ApiVersion,
        body: Vec<u8>,
        kind: OperationKind,
    ) -> Result<RawResponse> {
        let url = self.graphql_url(version);
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
                429 => {
                    let asked = retry_after(&response);
                    let throttled = Error::Throttled {
                        retries,
                        cost: None,
                        retry_after: asked,
                    };
                    if !may_retry {
                        return Err(throttled);
                    }
                    // `max_delay` caps the backoff, not the wait Shopify asks for;
                    // a wait longer than `max_retry_after` goes back to the caller.
                    let wait = match asked {
                        Some(wait) if wait > self.retry.max_retry_after() => {
                            tracing::debug!("Shopify answered 429 with a Retry-After of {wait:?}");
                            return Err(throttled);
                        }
                        Some(wait) => self.retry.retry_after_wait(wait),
                        None => self.retry.backoff(retries),
                    };
                    tracing::debug!("Shopify answered 429: retrying in {wait:?}");
                    tokio::time::sleep(wait).await;
                    retries += 1;
                    continue;
                }
                500..=599 if may_retry && !kind.is_mutation() => {
                    tracing::debug!("Shopify answered {}: retrying the query", response.status);
                    tokio::time::sleep(self.retry.backoff(retries)).await;
                    retries += 1;
                    continue;
                }
                _ => return Err(Error::http(&url, &response)),
            }

            let raw: RawResponse = crate::decode::from_slice(&response.body)
                .map_err(|failure| failure.into_error())?;
            if raw.is_throttled() {
                let cost = raw.cost();
                if !may_retry {
                    return Err(Error::Throttled {
                        retries,
                        cost,
                        retry_after: None,
                    });
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

/// The wait a `Retry-After` header asks for: a number of seconds, as Shopify
/// sends, or an HTTP date in its usual form, counted from now. `None` for a
/// value that is neither. A date already past is no wait; a wait too long
/// for a `Duration` is the longest one.
fn retry_after(response: &crate::transport::HttpResponse) -> Option<std::time::Duration> {
    retry_after_from(response.header("Retry-After")?, chrono::Utc::now())
}

fn retry_after_from(
    value: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<std::time::Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<f64>() {
        if seconds.is_nan() || seconds < 0.0 {
            return None;
        }
        return Some(
            std::time::Duration::try_from_secs_f64(seconds).unwrap_or(std::time::Duration::MAX),
        );
    }
    let date = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some((date.to_utc() - now).to_std().unwrap_or_default())
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::retry_after_from;

    #[test]
    fn retry_after_reads_seconds_and_http_dates() {
        let now = chrono::DateTime::parse_from_rfc3339("1994-11-06T08:47:37Z")
            .unwrap()
            .to_utc();
        let read = |value| retry_after_from(value, now);
        assert_eq!(read("120"), Some(Duration::from_secs(120)));
        assert_eq!(read(" 2.5 "), Some(Duration::from_millis(2500)));
        assert_eq!(read("-5"), None);
        // An HTTP date, two minutes after `now`.
        assert_eq!(
            read("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(Duration::from_secs(120))
        );
        // The obsolete forms are not read: the client backs off instead.
        assert_eq!(read("Sunday, 06-Nov-94 08:49:37 GMT"), None);
        // A date already past is no wait.
        assert_eq!(read("Sun, 06 Nov 1994 08:00:00 GMT"), Some(Duration::ZERO));
        assert_eq!(read("soon"), None);
    }
}
