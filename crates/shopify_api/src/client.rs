//! The client of one shop.

use std::sync::Arc;
use std::time::Duration;

use crate::auth::{Endpoint, Tokens};
use crate::transport::{Transport, default_transport};
use crate::{ApiVersion, Auth, Error, Result, ShopDomain, TokenData, TokenStore};

/// The `User-Agent` of every request, unless [`ShopifyBuilder::user_agent`] sets another.
pub const USER_AGENT: &str = concat!("shopify_api/", env!("CARGO_PKG_VERSION"), " (Rust)");

/// The Admin API of one shop.
///
/// A client is cheap to clone: clones share the transport and the token, so
/// that one renewal serves them all.
///
/// ```no_run
/// use shopify_api::{Shopify, graphql};
///
/// # async fn run() -> shopify_api::Result<()> {
/// let shopify = Shopify::new("my-shop", shopify_api::Auth::access_token("shpat_…"))?;
///
/// let data = shopify
///     .execute(graphql!("query { shop { name currencyCode } }"))
///     .await?;
/// println!("{} sells in {}", data.shop.name, data.shop.currency_code);
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Shopify {
    pub(crate) shared: Arc<Shared>,
    /// The version every request goes to, when one was chosen.
    pinned: Option<ApiVersion>,
    pub(crate) retry: RetryPolicy,
}

pub(crate) struct Shared {
    pub endpoint: Arc<Endpoint>,
    pub tokens: Tokens,
}

impl std::fmt::Debug for Shopify {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Shopify")
            .field("shop", &self.shared.endpoint.shop)
            .field("api_version", &self.pinned)
            .field("retry", &self.retry)
            .finish_non_exhaustive()
    }
}

impl Shopify {
    /// A client of `shop` (`my-shop` or `my-shop.myshopify.com`) with the
    /// default transport, retries and API version.
    pub fn new(shop: impl AsRef<str>, auth: Auth) -> Result<Self> {
        Self::builder(shop).auth(auth).build()
    }

    pub fn builder(shop: impl AsRef<str>) -> ShopifyBuilder {
        ShopifyBuilder {
            shop: shop.as_ref().to_string(),
            auth: None,
            api_version: None,
            transport: None,
            retry: RetryPolicy::default(),
            token_store: None,
            refresh_leeway: Duration::from_secs(5 * 60),
            user_agent: USER_AGENT.to_string(),
        }
    }

    pub fn shop(&self) -> &ShopDomain {
        &self.shared.endpoint.shop
    }

    /// The version requests go to: the one pinned with
    /// [`ShopifyBuilder::api_version`], else [`ApiVersion::DEFAULT`].
    ///
    /// Without a pinned version, an operation of [`graphql!`](crate::graphql!)
    /// goes to the version of the schema it was checked against.
    pub fn api_version(&self) -> ApiVersion {
        self.pinned.unwrap_or_default()
    }

    /// The version pinned with [`ShopifyBuilder::api_version`], if any.
    pub fn pinned_api_version(&self) -> Option<ApiVersion> {
        self.pinned
    }

    /// A clone that sends every request to `version`, sharing the token.
    pub fn with_api_version(&self, version: ApiVersion) -> Self {
        Self {
            pinned: Some(version),
            ..self.clone()
        }
    }

    /// A clone with other retries, sharing the token.
    pub fn with_retry(&self, retry: RetryPolicy) -> Self {
        Self {
            retry,
            ..self.clone()
        }
    }

    /// `https://{shop}/admin/api/{version}/graphql.json`.
    pub fn graphql_url(&self, version: ApiVersion) -> String {
        format!(
            "https://{}/admin/api/{version}/graphql.json",
            self.shared.endpoint.shop
        )
    }

    /// The access token requests send, obtained or renewed first when it is
    /// missing or about to expire.
    pub async fn access_token(&self) -> Result<String> {
        self.shared.tokens.get(&self.shared.endpoint).await
    }

    /// The token the client holds, without asking Shopify for one: to store
    /// it, or to read its scopes.
    pub async fn token(&self) -> Option<TokenData> {
        self.shared.tokens.current().await
    }

    pub(crate) fn endpoint(&self) -> &Arc<Endpoint> {
        &self.shared.endpoint
    }

    /// The version `operation` goes to: the pinned one, else the one its
    /// schema was checked against.
    pub(crate) fn version_for(&self, checked_against: Option<&'static str>) -> Result<ApiVersion> {
        match (self.pinned, checked_against) {
            (Some(pinned), Some(checked)) => {
                if pinned.to_string() != checked {
                    warn_mismatch(pinned, checked);
                }
                Ok(pinned)
            }
            (Some(pinned), None) => Ok(pinned),
            (None, Some(checked)) => ApiVersion::new(checked),
            (None, None) => Ok(ApiVersion::DEFAULT),
        }
    }
}

/// Warns once per pair of versions that an operation goes to another
/// version than the one it was checked against.
fn warn_mismatch(pinned: ApiVersion, checked: &'static str) {
    static WARNED: std::sync::Mutex<Vec<(ApiVersion, &'static str)>> =
        std::sync::Mutex::new(Vec::new());
    let mut warned = WARNED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if !warned.contains(&(pinned, checked)) {
        warned.push((pinned, checked));
        tracing::warn!(
            "an operation checked against the Admin API {checked} is sent to {pinned}: \
             set `api-version = \"{pinned}\"` in `[package.metadata.shopify_api]` of Cargo.toml, \
             or let the client follow the schema by not pinning a version"
        );
    }
}

/// How a client retries what Shopify did not run.
///
/// A request Shopify throttled (`THROTTLED`, or a `429`) is sent again after
/// the time its cost says the bucket needs. A query is also sent again after
/// a `5xx` or a network failure, with exponential backoff; a mutation is not,
/// since it may have run.
///
/// A `429` waits for its `Retry-After`, however longer than
/// [`max_delay`](Self::max_delay) it is: that cap is for the backoff the client
/// computes. A `Retry-After` longer than
/// [`max_retry_after`](Self::max_retry_after) (five minutes by default) is not
/// waited for: the request ends in [`Error::Throttled`](crate::Error::Throttled),
/// with the wait Shopify asked for in its `retry_after`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    max_retries: u32,
    base_delay: Duration,
    max_delay: Duration,
    max_retry_after: Duration,
    /// `false` only for `testing::client`: a `Retry-After` is not slept through.
    wait_for_retry_after: bool,
}

impl RetryPolicy {
    /// At most `max_retries` retries, from one second up to thirty seconds
    /// apart, or as long as a `Retry-After` of up to five minutes asks.
    pub const fn new(max_retries: u32) -> Self {
        Self {
            max_retries,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(30),
            max_retry_after: Duration::from_secs(300),
            wait_for_retry_after: true,
        }
    }

    /// Never retries.
    pub const fn none() -> Self {
        Self::new(0)
    }

    /// The first backoff, doubled at each retry.
    pub const fn with_base_delay(mut self, delay: Duration) -> Self {
        self.base_delay = delay;
        self
    }

    /// The longest backoff between two attempts. A `Retry-After` from
    /// Shopify is not capped by it: see [`with_max_retry_after`](Self::with_max_retry_after).
    pub const fn with_max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    /// The longest `Retry-After` the client waits for before sending a `429`
    /// again. A longer one ends the request in
    /// [`Error::Throttled`](crate::Error::Throttled) at once.
    pub const fn with_max_retry_after(mut self, wait: Duration) -> Self {
        self.max_retry_after = wait;
        self
    }

    /// Retries a `429` at once instead of waiting for its `Retry-After`, for
    /// tests with a mocked Shopify. A `Retry-After` beyond `max_retry_after`
    /// still ends in [`Error::Throttled`](crate::Error::Throttled).
    #[cfg_attr(not(feature = "testing"), allow(dead_code))]
    pub(crate) const fn without_retry_after_waits(mut self) -> Self {
        self.wait_for_retry_after = false;
        self
    }

    /// The wait before retrying a `429` whose `Retry-After` asked for `wait`.
    pub(crate) const fn retry_after_wait(&self, wait: Duration) -> Duration {
        if self.wait_for_retry_after {
            wait
        } else {
            Duration::ZERO
        }
    }

    pub const fn max_retries(&self) -> u32 {
        self.max_retries
    }

    /// The longest backoff between two attempts.
    pub const fn max_delay(&self) -> Duration {
        self.max_delay
    }

    /// The longest `Retry-After` the client waits for.
    pub const fn max_retry_after(&self) -> Duration {
        self.max_retry_after
    }

    /// The wait before retry number `retry` (from 0): exponential, with up
    /// to a quarter of jitter, capped at the longest wait.
    pub(crate) fn backoff(&self, retry: u32) -> Duration {
        let exponential = self.base_delay.saturating_mul(1u32 << retry.min(16));
        let jitter = exponential.mul_f64(jitter_fraction() * 0.25);
        exponential.saturating_add(jitter).min(self.max_delay)
    }

    /// The wait before sending a throttled request again: until the bucket
    /// holds its cost, as Shopify reported it, else the backoff.
    pub(crate) fn throttled(&self, retry: u32, cost: Option<crate::graphql::Cost>) -> Duration {
        match cost.map(|cost| cost.seconds_until_available()) {
            Some(seconds) if seconds > 0.0 => {
                // A little more than needed: the bucket refills continuously.
                // What does not fit in a `Duration` is the longest wait.
                Duration::try_from_secs_f64(seconds + 0.1)
                    .map_or(self.max_delay, |wait| wait.min(self.max_delay))
            }
            _ => self.backoff(retry),
        }
    }
}

impl Default for RetryPolicy {
    /// Four retries.
    fn default() -> Self {
        Self::new(4)
    }
}

/// A fraction in `[0, 1)`, from the clock: retries need no better randomness.
fn jitter_fraction() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or_default();
    f64::from(nanos % 1000) / 1000.0
}

/// Builds a [`Shopify`] client.
///
/// ```
/// use std::time::Duration;
/// use shopify_api::{ApiVersion, RetryPolicy, Shopify};
///
/// let shopify = Shopify::builder("my-shop")
///     .client_credentials("client-id", "client-secret")
///     .api_version(ApiVersion::V2026_10)
///     .retry(RetryPolicy::new(6).with_max_delay(Duration::from_secs(10)))
///     .build()?;
/// # Ok::<(), shopify_api::Error>(())
/// ```
#[must_use]
pub struct ShopifyBuilder {
    shop: String,
    auth: Option<Auth>,
    api_version: Option<ApiVersion>,
    transport: Option<Arc<dyn Transport>>,
    retry: RetryPolicy,
    token_store: Option<Arc<dyn TokenStore>>,
    refresh_leeway: Duration,
    user_agent: String,
}

impl std::fmt::Debug for ShopifyBuilder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ShopifyBuilder")
            .field("shop", &self.shop)
            .field("auth", &self.auth)
            .field("api_version", &self.api_version)
            .field("retry", &self.retry)
            .finish_non_exhaustive()
    }
}

impl ShopifyBuilder {
    pub fn auth(mut self, auth: Auth) -> Self {
        self.auth = Some(auth);
        self
    }

    /// [`Auth::access_token`].
    pub fn access_token(self, token: impl Into<String>) -> Self {
        self.auth(Auth::access_token(token))
    }

    /// [`Auth::client_credentials`].
    pub fn client_credentials(
        self,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        self.auth(Auth::client_credentials(client_id, client_secret))
    }

    /// Sends every request to `version`, instead of the version each
    /// [`graphql!`](crate::graphql!) operation was checked against.
    pub fn api_version(mut self, version: ApiVersion) -> Self {
        self.api_version = Some(version);
        self
    }

    /// Sends the requests through `transport`, such as a
    /// [`MockTransport`](crate::testing::MockTransport) in tests.
    pub fn transport(mut self, transport: impl Transport) -> Self {
        self.transport = Some(Arc::new(transport));
        self
    }

    /// [`ShopifyBuilder::transport`], for a transport already shared.
    pub fn shared_transport(mut self, transport: Arc<dyn Transport>) -> Self {
        self.transport = Some(transport);
        self
    }

    pub fn retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    /// Saves the tokens the client obtains or renews.
    pub fn token_store(mut self, store: impl TokenStore) -> Self {
        self.token_store = Some(Arc::new(store));
        self
    }

    /// How long before it expires a token is renewed: five minutes by default.
    pub fn refresh_leeway(mut self, leeway: Duration) -> Self {
        self.refresh_leeway = leeway;
        self
    }

    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    pub fn build(self) -> Result<Shopify> {
        let shop = ShopDomain::new(&self.shop)?;
        let auth = self.auth.ok_or_else(|| {
            Error::Auth(
                "no credentials: call `access_token`, `client_credentials` or `auth`".to_string(),
            )
        })?;
        let transport = match self.transport {
            Some(transport) => transport,
            None => default_transport()?,
        };
        let leeway =
            chrono::Duration::from_std(self.refresh_leeway).unwrap_or(chrono::Duration::MAX);
        Ok(Shopify {
            shared: Arc::new(Shared {
                endpoint: Arc::new(Endpoint {
                    transport,
                    shop,
                    user_agent: self.user_agent,
                }),
                tokens: Tokens::new(auth, self.token_store, leeway),
            }),
            pinned: self.api_version,
            retry: self.retry,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_up_to_the_longest_wait() {
        let retry = RetryPolicy::new(5)
            .with_base_delay(Duration::from_millis(100))
            .with_max_delay(Duration::from_secs(1));
        let within = |retry_number: u32, low: u64, high: u64| {
            let wait = retry.backoff(retry_number);
            assert!(
                wait >= Duration::from_millis(low) && wait <= Duration::from_millis(high),
                "retry {retry_number}: {wait:?}"
            );
        };
        within(0, 100, 125);
        within(1, 200, 250);
        within(2, 400, 500);
        within(4, 1000, 1000);
        within(30, 1000, 1000);
    }

    #[test]
    fn a_throttled_request_waits_for_its_cost() {
        let retry = RetryPolicy::default();
        let cost = crate::graphql::Cost {
            requested_query_cost: 500.0,
            actual_query_cost: None,
            throttle_status: crate::graphql::ThrottleStatus {
                maximum_available: 2000.0,
                currently_available: 300.0,
                restore_rate: 100.0,
            },
        };
        assert_eq!(retry.throttled(0, Some(cost)), Duration::from_millis(2100));
        let far = crate::graphql::Cost {
            requested_query_cost: 20000.0,
            ..cost
        };
        assert_eq!(retry.throttled(0, Some(far)), Duration::from_secs(30));
    }

    #[cfg(feature = "testing")]
    #[test]
    fn the_version_follows_the_schema_unless_one_is_pinned() {
        let mock = crate::testing::MockTransport::new();
        let shopify = Shopify::builder("shop")
            .access_token("t")
            .transport(mock)
            .build()
            .unwrap();
        assert_eq!(shopify.version_for(None).unwrap(), ApiVersion::DEFAULT);
        assert_eq!(
            shopify.version_for(Some("2026-07")).unwrap(),
            ApiVersion::V2026_07
        );
        assert!(shopify.version_for(Some("2025-01")).is_err());
        let pinned = shopify.with_api_version(ApiVersion::UNSTABLE);
        assert_eq!(
            pinned.version_for(Some("2026-07")).unwrap(),
            ApiVersion::UNSTABLE
        );
        assert_eq!(pinned.api_version(), ApiVersion::UNSTABLE);
        assert_eq!(shopify.api_version(), ApiVersion::DEFAULT);
        assert_eq!(
            shopify.graphql_url(ApiVersion::V2026_10),
            "https://shop.myshopify.com/admin/api/2026-10/graphql.json"
        );
    }

    #[test]
    fn building_needs_a_shop_and_credentials() {
        assert!(matches!(
            Shopify::builder("not a shop").access_token("t").build(),
            Err(Error::InvalidShop(_))
        ));
        assert!(matches!(
            Shopify::builder("shop").build(),
            Err(Error::Auth(_))
        ));
    }
}
