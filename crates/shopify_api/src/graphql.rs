//! Running GraphQL operations: typed ones from [`graphql!`](crate::graphql!),
//! untyped ones from any text.

pub(crate) mod operation;
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
            data: crate::decode::from_value(data)?,
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
        crate::decode::from_value(self.graphql(query, variables).await?)
    }

    /// Runs any query or mutation and returns the answer as Shopify sent
    /// it, `errors` included: for an operation whose partial `data` matters.
    /// A throttled request is still retried.
    pub async fn graphql_raw(&self, query: &str, variables: impl Serialize) -> Result<RawResponse> {
        let variables = crate::graphql::operation::variables_of(variables)?;
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
        let body = serde_json::to_vec(body).map_err(|error| Error::variables(None, &error))?;
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

/// The wait a `Retry-After` header asks for: a number of seconds, or an
/// HTTP date (RFC 9110), counted from now. `None` for a value that is
/// neither. A date already past is no wait; a wait too long for a `Duration`
/// is the longest one.
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
    let date = http_date(value, now)?;
    Some((date - now).to_std().unwrap_or_default())
}

/// An HTTP date in any of its three forms: `Sun, 06 Nov 1994 08:49:37 GMT`,
/// the obsolete `Sunday, 06-Nov-94 08:49:37 GMT` and `Sun Nov  6 08:49:37 1994`.
///
/// The two-digit year of the second form is read as RFC 9110 (§5.6.7) says:
/// a date more than 50 years after `now` is the most recent past year with
/// the same last two digits.
fn http_date(
    value: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{DateTime, NaiveDateTime, Utc};
    if let Ok(date) = DateTime::parse_from_rfc2822(value) {
        return Some(date.with_timezone(&Utc));
    }
    if let Some(date) = rfc850_date(value, now) {
        return Some(date);
    }
    NaiveDateTime::parse_from_str(value, "%a %b %e %H:%M:%S %Y")
        .ok()
        .map(|date| date.and_utc())
}

/// `Sunday, 06-Nov-94 08:49:37 GMT`, its century chosen from `now`.
fn rfc850_date(
    value: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{Datelike, Months, NaiveDateTime, Timelike};
    let (weekday, rest) = value.split_once(", ")?;
    let (day, rest) = rest.split_once('-')?;
    let (month, rest) = rest.split_once('-')?;
    let (year, time) = rest.split_once(' ')?;
    if year.len() != 2 || !year.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let two_digits: i32 = year.parse().ok()?;
    // The day, month and time, read in a leap year so that 29 February
    // reads: the century is chosen before the date is checked.
    let parts = NaiveDateTime::parse_from_str(
        &format!("{day}-{month}-2000 {time}"),
        "%d-%b-%Y %H:%M:%S GMT",
    )
    .ok()?;
    let latest = now.checked_add_months(Months::new(50 * 12))?;
    let at = |date: &NaiveDateTime| (date.month(), date.day(), date.num_seconds_from_midnight());
    // The latest year ending in these digits that is not more than 50 years ahead.
    let mut full_year = now.year() - now.year().rem_euclid(100) + 100 + two_digits;
    while (full_year, at(&parts)) > (latest.year(), at(&latest.naive_utc())) {
        full_year -= 100;
    }
    let date = parts.with_year(full_year)?.and_utc();
    let named = date.format("%A").to_string();
    (named == weekday).then_some(date)
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
        // The three forms of RFC 9110, two minutes after `now`.
        assert_eq!(
            read("Sun, 06 Nov 1994 08:49:37 GMT"),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            read("Sunday, 06-Nov-94 08:49:37 GMT"),
            Some(Duration::from_secs(120))
        );
        assert_eq!(
            read("Sun Nov  6 08:49:37 1994"),
            Some(Duration::from_secs(120))
        );
        // RFC 850's two-digit years: never more than 50 years ahead (RFC
        // 9110 §5.6.7).
        let in_2026 = chrono::DateTime::parse_from_rfc3339("2026-10-22T12:00:00Z")
            .unwrap()
            .to_utc();
        let year = |value| {
            use chrono::Datelike;
            super::http_date(value, in_2026).map(|date| date.year())
        };
        assert_eq!(year("Friday, 06-Nov-26 08:49:37 GMT"), Some(2026));
        assert_eq!(year("Wednesday, 06-Nov-69 08:49:37 GMT"), Some(2069));
        assert_eq!(year("Wednesday, 21-Oct-76 08:49:37 GMT"), Some(2076));
        // A day after `now` plus 50 years is in the past century.
        assert_eq!(year("Saturday, 23-Oct-76 08:49:37 GMT"), Some(1976));
        assert_eq!(year("Saturday, 06-Nov-99 08:49:37 GMT"), Some(1999));
        assert_eq!(year("Thursday, 06-Nov-70 08:49:37 GMT"), Some(2070));
        assert_eq!(year("Monday, 06-Nov-00 08:49:37 GMT"), Some(2000));
        // 29 February: the century first (2100 is no leap year, 2000 is).
        assert_eq!(year("Tuesday, 29-Feb-00 08:49:37 GMT"), Some(2000));
        assert_eq!(
            retry_after_from("Tuesday, 29-Feb-00 08:49:37 GMT", in_2026),
            Some(Duration::ZERO)
        );
        assert_eq!(year("Saturday, 29-Feb-76 08:49:37 GMT"), Some(2076));
        // No year ending in `01` has a 29 February.
        assert_eq!(year("Thursday, 29-Feb-01 08:49:37 GMT"), None);
        // A weekday that does not match its date is no date.
        assert_eq!(year("Monday, 06-Nov-69 08:49:37 GMT"), None);
        assert_eq!(year("Sunday, 06-Nov-1994 08:49:37 GMT"), None);
        // In 2069, `69` is this year, not a century ahead: an hour from now.
        let in_2069 = chrono::DateTime::parse_from_rfc3339("2069-11-06T07:49:37Z")
            .unwrap()
            .to_utc();
        assert_eq!(
            retry_after_from("Wednesday, 06-Nov-69 08:49:37 GMT", in_2069),
            Some(Duration::from_secs(3600))
        );
        // In 2099, `01` is two years ahead, not 98 years ago.
        let in_2099 = chrono::DateTime::parse_from_rfc3339("2099-01-01T00:00:00Z")
            .unwrap()
            .to_utc();
        assert_eq!(
            super::http_date("Saturday, 01-Jan-01 00:00:00 GMT", in_2099)
                .map(|date| date.to_rfc3339()),
            Some("2101-01-01T00:00:00+00:00".to_string())
        );
        // A date already past is no wait.
        assert_eq!(read("Sun, 06 Nov 1994 08:00:00 GMT"), Some(Duration::ZERO));
        assert_eq!(read("soon"), None);
    }
}
