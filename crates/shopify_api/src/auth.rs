//! Access tokens: a static token, the client credentials grant, or an
//! expiring offline token renewed with its refresh token.

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::transport::{BoxFuture, HttpRequest, Method, Transport};
use crate::{Error, Result, ShopDomain};

/// How a [`Shopify`](crate::Shopify) client gets its access token.
///
/// ```
/// use shopify_api::{Auth, TokenData};
///
/// // A token of a custom app, or one your app stored at installation.
/// let auth = Auth::access_token("shpat_…");
///
/// // A token Shopify gives for 24 hours to an app and a store of the same
/// // organization, asked for again before it expires.
/// let auth = Auth::client_credentials("client-id", "client-secret");
///
/// // An expiring offline token, renewed with its refresh token.
/// let mut stored = TokenData::new("shpat_…");
/// stored.refresh_token = Some("shprt_…".to_string());
/// let auth = Auth::expiring_token("client-id", "client-secret", stored);
/// ```
#[derive(Clone)]
pub struct Auth {
    kind: Kind,
    token: Option<TokenData>,
}

#[derive(Clone)]
enum Kind {
    /// The token is the token: nothing renews it.
    Static,
    /// The client credentials grant gives a new token.
    ClientCredentials {
        client_id: String,
        client_secret: String,
    },
    /// The refresh token of the current token gives a new token.
    Refresh {
        client_id: String,
        client_secret: String,
    },
}

impl Auth {
    /// A token that is used as it is: an Admin API token of a custom app, or
    /// a non-expiring offline token.
    pub fn access_token(token: impl Into<String>) -> Self {
        Self::token(TokenData::new(token))
    }

    /// A token used as it is, with what is known of it (its scopes, when it
    /// expires). Nothing renews it.
    pub fn token(token: TokenData) -> Self {
        Self {
            kind: Kind::Static,
            token: Some(token),
        }
    }

    /// The client credentials grant: Shopify gives a token, valid about 24
    /// hours, to an app for a store of the organization that owns the app.
    /// The client asks for one at its first request, and again five minutes
    /// before it expires or when Shopify refuses it.
    pub fn client_credentials(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        Self {
            kind: Kind::ClientCredentials {
                client_id: client_id.into(),
                client_secret: client_secret.into(),
            },
            token: None,
        }
    }

    /// An expiring offline token and its refresh token: the client renews
    /// it five minutes before it expires, or when Shopify refuses it. Shopify
    /// gives a new refresh token each time: save it with a
    /// [`TokenStore`].
    pub fn expiring_token(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        token: TokenData,
    ) -> Self {
        Self {
            kind: Kind::Refresh {
                client_id: client_id.into(),
                client_secret: client_secret.into(),
            },
            token: Some(token),
        }
    }

    /// Starts from a token you stored, such as the last client credentials
    /// token, instead of asking Shopify for one at the first request.
    pub fn with_token(mut self, token: TokenData) -> Self {
        self.token = Some(token);
        self
    }
}

impl fmt::Debug for Auth {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            Kind::Static => "Static",
            Kind::ClientCredentials { .. } => "ClientCredentials",
            Kind::Refresh { .. } => "Refresh",
        };
        formatter
            .debug_struct("Auth")
            .field("kind", &kind)
            .field("token", &self.token)
            .finish_non_exhaustive()
    }
}

/// An access token and what Shopify said about it.
///
/// It serializes to JSON, to be stored and given back with
/// [`Auth::with_token`] or [`Auth::expiring_token`]. `Debug` hides the tokens.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct TokenData {
    pub access_token: String,
    /// The access scopes granted, comma-separated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// When the access token expires; `None` when it does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token_expires_at: Option<DateTime<Utc>>,
    /// For an online token: the staff member it acts for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub associated_user: Option<AssociatedUser>,
    /// For an online token: the scopes of that staff member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub associated_user_scope: Option<String>,
}

impl TokenData {
    /// A token of which nothing else is known.
    pub fn new(access_token: impl Into<String>) -> Self {
        Self {
            access_token: access_token.into(),
            scope: None,
            expires_at: None,
            refresh_token: None,
            refresh_token_expires_at: None,
            associated_user: None,
            associated_user_scope: None,
        }
    }

    /// The access token expires within `leeway`, or has expired.
    pub fn expires_within(&self, leeway: chrono::Duration) -> bool {
        self.expires_at
            .is_some_and(|expires_at| match Utc::now().checked_add_signed(leeway) {
                Some(limit) => expires_at <= limit,
                // A leeway beyond the calendar: everything expires within it.
                None => true,
            })
    }

    /// The refresh token is still valid, as far as is known.
    pub fn can_refresh(&self) -> bool {
        self.refresh_token.is_some()
            && self
                .refresh_token_expires_at
                .is_none_or(|expires_at| Utc::now() < expires_at)
    }

    /// The granted scopes.
    pub fn scopes(&self) -> Vec<&str> {
        self.scope
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|scope| !scope.is_empty())
            .collect()
    }
}

impl fmt::Debug for TokenData {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TokenData")
            .field("access_token", &"[redacted]")
            .field("scope", &self.scope)
            .field("expires_at", &self.expires_at)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("refresh_token_expires_at", &self.refresh_token_expires_at)
            .field("associated_user", &self.associated_user)
            .field("associated_user_scope", &self.associated_user_scope)
            .finish()
    }
}

/// The staff member an online token acts for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct AssociatedUser {
    pub id: u64,
    #[serde(default)]
    pub first_name: Option<String>,
    #[serde(default)]
    pub last_name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub email_verified: Option<bool>,
    #[serde(default)]
    pub account_owner: Option<bool>,
    #[serde(default)]
    pub locale: Option<String>,
    #[serde(default)]
    pub collaborator: Option<bool>,
}

/// Where a client saves the tokens it obtains or renews, so that a restart
/// reuses them. An expiring offline token comes with a new refresh token
/// each time: without a store, a restart is left with one that no longer works.
///
/// Saves run one at a time, in the order of the renewals, in a task of their
/// own: a request that is cancelled does not cancel its save. A store must
/// not call the client whose token it saves.
///
/// ```
/// use shopify_api::transport::BoxFuture;
/// use shopify_api::{ShopDomain, TokenData, TokenStore};
///
/// struct Database;
///
/// impl TokenStore for Database {
///     fn save<'a>(
///         &'a self,
///         shop: &'a ShopDomain,
///         token: &'a TokenData,
///     ) -> BoxFuture<'a, Result<(), Box<dyn std::error::Error + Send + Sync>>> {
///         Box::pin(async move {
///             let json = serde_json::to_string(token)?;
///             // UPDATE installations SET token = $json WHERE shop = $shop
///             let _ = (shop, json);
///             Ok(())
///         })
///     }
/// }
/// ```
pub trait TokenStore: Send + Sync + 'static {
    fn save<'a>(
        &'a self,
        shop: &'a ShopDomain,
        token: &'a TokenData,
    ) -> BoxFuture<'a, std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>>;
}

// ─── Token endpoint ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    refresh_token_expires_in: Option<i64>,
    #[serde(default)]
    associated_user: Option<AssociatedUser>,
    #[serde(default)]
    associated_user_scope: Option<String>,
}

impl TokenResponse {
    fn into_token(self) -> TokenData {
        // Whatever Shopify says, a date that exists: never in the past, at most the last one.
        let at = |seconds: i64| {
            chrono::Duration::try_seconds(seconds.max(0))
                .and_then(|duration| Utc::now().checked_add_signed(duration))
                .unwrap_or(DateTime::<Utc>::MAX_UTC)
        };
        TokenData {
            access_token: self.access_token,
            scope: self.scope,
            expires_at: self.expires_in.map(at),
            refresh_token: self.refresh_token,
            refresh_token_expires_at: self.refresh_token_expires_in.map(at),
            associated_user: self.associated_user,
            associated_user_scope: self.associated_user_scope,
        }
    }
}

pub(crate) fn token_url(shop: &ShopDomain) -> String {
    format!("https://{shop}/admin/oauth/access_token")
}

/// How the fields of a token request travel.
#[derive(Clone, Copy)]
pub(crate) enum Encoding {
    /// `application/x-www-form-urlencoded`, as Shopify documents the client credentials grant.
    Form,
    /// `application/json`, as Shopify's libraries send the code and token exchanges.
    Json,
}

/// Asks the token endpoint of `shop` for a token. A refusal says what
/// Shopify answered, never what was sent.
pub(crate) async fn request_token(
    transport: &dyn Transport,
    shop: &ShopDomain,
    user_agent: &str,
    encoding: Encoding,
    fields: &[(&str, &str)],
) -> Result<TokenData> {
    let url = token_url(shop);
    let request = HttpRequest::new(Method::Post, &url)
        .with_header("Accept", "application/json")
        .with_header("User-Agent", user_agent)
        .with_timeout(std::time::Duration::from_secs(30));
    let request = match encoding {
        Encoding::Form => request.with_form(fields),
        Encoding::Json => {
            let body: serde_json::Map<String, serde_json::Value> = fields
                .iter()
                .map(|(name, value)| (name.to_string(), serde_json::Value::from(*value)))
                .collect();
            request.with_json(&serde_json::Value::Object(body))
        }
    };
    let response = transport
        .send(request)
        .await
        .map_err(|source| Error::transport(&url, source))?;
    if !response.is_success() {
        // What the endpoint wrote is kept apart: it may echo what it was sent.
        let body: serde_json::Value = serde_json::from_slice(&response.body).unwrap_or_default();
        let code = body
            .get("error")
            .and_then(serde_json::Value::as_str)
            .map(oauth_error_code);
        let description = body
            .get("error_description")
            .and_then(serde_json::Value::as_str);
        let detail = match (
            body.get("error").and_then(serde_json::Value::as_str),
            description,
        ) {
            (Some(code), Some(description)) => format!("{code}: {description}"),
            (Some(code), None) => code.to_string(),
            _ => String::from_utf8_lossy(&response.body).into_owned(),
        };
        let message = match code {
            Some(code) => format!(
                "the token endpoint of {shop} answered {} ({code})",
                response.status
            ),
            None => format!("the token endpoint of {shop} answered {}", response.status),
        };
        return Err(Error::auth_with_detail(message, detail));
    }
    let parsed: TokenResponse = crate::decode::from_slice(&response.body).map_err(|failure| {
        Error::auth_with_detail(
            format!(
                "the token endpoint of {shop} answered an invalid token: {} at `{}`",
                failure.message, failure.path
            ),
            failure.detail,
        )
    })?;
    Ok(parsed.into_token())
}

// ─── Token of a client ───────────────────────────────────────────────

/// How long a failed renewal ahead of expiry holds back the next one, while
/// the current token still works.
pub(crate) const RENEWAL_BACKOFF: Duration = Duration::from_secs(30);

/// How long a failed renewal is the answer of the requests that wait for a
/// token, instead of each asking Shopify again.
pub(crate) const FAILURE_HOLD: Duration = Duration::from_secs(5);

/// What the client needs to ask for a token.
pub(crate) struct Endpoint {
    pub transport: Arc<dyn Transport>,
    pub shop: ShopDomain,
    pub user_agent: String,
}

/// The token of a client and of its clones.
///
/// One renewal runs at a time, in a task of its own: a caller that is
/// cancelled (a request handler dropped, a timeout) does not cancel it, so a
/// refresh token Shopify rotated is never lost halfway.
pub(crate) struct Tokens {
    kind: Arc<Kind>,
    state: Arc<Mutex<State>>,
    /// Saves run in the order of the renewals, outside the lock of the state.
    saves: Arc<Mutex<()>>,
    store: Option<Arc<dyn TokenStore>>,
    leeway: chrono::Duration,
}

struct State {
    token: Option<TokenData>,
    /// When the last renewal failed, and why.
    failure: Option<(Instant, String)>,
}

impl State {
    /// The access token, if it has not expired.
    fn valid(&self) -> Option<String> {
        self.token
            .as_ref()
            .filter(|token| !token.expires_within(chrono::Duration::zero()))
            .map(|token| token.access_token.clone())
    }

    /// The last failure, if it is recent enough to be the answer.
    fn held_failure(&self) -> Option<Error> {
        let (at, message) = self.failure.as_ref()?;
        (at.elapsed() < FAILURE_HOLD)
            .then(|| Error::auth(format!("{message} (asked again in a few seconds)")))
    }
}

impl Tokens {
    pub fn new(auth: Auth, store: Option<Arc<dyn TokenStore>>, leeway: chrono::Duration) -> Self {
        Self {
            kind: Arc::new(auth.kind),
            state: Arc::new(Mutex::new(State {
                token: auth.token,
                failure: None,
            })),
            saves: Arc::new(Mutex::new(())),
            store,
            leeway,
        }
    }

    /// The token to send, obtained or renewed first when it is missing or
    /// about to expire.
    ///
    /// When a renewal ahead of expiry fails, the token that has not expired
    /// yet is still sent, and the renewal is not tried again for
    /// [`RENEWAL_BACKOFF`].
    pub async fn get(&self, endpoint: &Arc<Endpoint>) -> Result<String> {
        let state = self.state.clone().lock_owned().await;
        let still_valid = state.valid();
        if let Some(token) = state.token.as_ref() {
            let renewable = !matches!(*self.kind, Kind::Static);
            let backing_off = state
                .failure
                .as_ref()
                .is_some_and(|(at, _)| at.elapsed() < RENEWAL_BACKOFF);
            if !renewable
                || !token.expires_within(self.leeway)
                || (backing_off && still_valid.is_some())
            {
                return Ok(token.access_token.clone());
            }
        }
        if still_valid.is_none()
            && let Some(error) = state.held_failure()
        {
            return Err(error);
        }
        match self.renew_locked(state, endpoint).await {
            Err(error) if !matches!(error, Error::TokenStore(_)) => match still_valid {
                Some(token) => {
                    tracing::warn!(
                        "renewing the token of {} ahead of its expiry failed, the current one is sent: {error}",
                        endpoint.shop
                    );
                    Ok(token)
                }
                None => Err(error),
            },
            outcome => outcome,
        }
    }

    /// A token to send again after Shopify refused `rejected`: the one
    /// another request already obtained, or a new one. `None` when nothing
    /// can renew it.
    pub async fn renew(&self, endpoint: &Arc<Endpoint>, rejected: &str) -> Result<Option<String>> {
        if matches!(*self.kind, Kind::Static) {
            return Ok(None);
        }
        let state = self.state.clone().lock_owned().await;
        if let Some(token) = state.token.as_ref()
            && token.access_token != rejected
        {
            return Ok(Some(token.access_token.clone()));
        }
        if let Some(error) = state.held_failure() {
            return Err(error);
        }
        self.renew_locked(state, endpoint).await.map(Some)
    }

    /// The current token, without asking Shopify for one.
    pub async fn current(&self) -> Option<TokenData> {
        self.state.lock().await.token.clone()
    }

    /// Obtains a new token in a task of its own, which keeps the lock of the
    /// state until the token is in it, and saves it in the order of the renewals.
    async fn renew_locked(
        &self,
        mut state: OwnedMutexGuard<State>,
        endpoint: &Arc<Endpoint>,
    ) -> Result<String> {
        if matches!(*self.kind, Kind::Static) {
            // A static client always starts with its token.
            return state
                .token
                .as_ref()
                .map(|token| token.access_token.clone())
                .ok_or_else(|| Error::auth("no access token"));
        }
        let kind = self.kind.clone();
        let endpoint = endpoint.clone();
        let store = self.store.clone();
        let saves = self.saves.clone();
        let renewal = tokio::spawn(async move {
            let token = match obtain(&kind, &state, &endpoint).await {
                Ok(token) => token,
                Err(error) => {
                    state.failure = Some((Instant::now(), error.to_string()));
                    return Err(error);
                }
            };
            let access_token = token.access_token.clone();
            state.token = Some(token.clone());
            state.failure = None;
            let turn = saves.lock_owned().await;
            drop(state);
            let saved = match &store {
                Some(store) => store
                    .save(&endpoint.shop, &token)
                    .await
                    .map_err(Error::TokenStore),
                None => Ok(()),
            };
            drop(turn);
            saved.map(|()| access_token)
        });
        renewal.await.map_err(|stopped| {
            // A panic's message is not this crate's to show.
            Error::auth(if stopped.is_panic() {
                "the renewal of the token panicked"
            } else {
                "the renewal of the token was cancelled"
            })
        })?
    }
}

/// An OAuth error code (RFC 6749, 5.2, and Shopify's) as an error shows it.
fn oauth_error_code(code: &str) -> &'static str {
    const CODES: [&str; 8] = [
        "shop_not_permitted",
        "invalid_request",
        "invalid_client",
        "invalid_grant",
        "unauthorized_client",
        "unsupported_grant_type",
        "invalid_scope",
        "invalid_subject_token",
    ];
    CODES
        .iter()
        .find(|known| **known == code)
        .copied()
        .unwrap_or("an error code this crate does not know")
}

/// Asks Shopify for a new token, as `kind` does.
async fn obtain(kind: &Kind, state: &State, endpoint: &Endpoint) -> Result<TokenData> {
    let transport = endpoint.transport.as_ref();
    match kind {
        Kind::Static => Err(Error::auth("a static token is not renewed")),
        Kind::ClientCredentials {
            client_id,
            client_secret,
        } => {
            let fields = [
                ("grant_type", "client_credentials"),
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
            ];
            request_token(
                transport,
                &endpoint.shop,
                &endpoint.user_agent,
                Encoding::Form,
                &fields,
            )
            .await
        }
        Kind::Refresh {
            client_id,
            client_secret,
        } => {
            let refresh_token = state
                .token
                .as_ref()
                .filter(|token| token.can_refresh())
                .and_then(|token| token.refresh_token.clone())
                .ok_or_else(|| {
                    Error::auth(format!(
                        "the token of {} expired and its refresh token is missing or expired: \
                         the app must get a new token (token exchange or OAuth)",
                        endpoint.shop
                    ))
                })?;
            let fields = [
                ("grant_type", "refresh_token"),
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
                ("refresh_token", refresh_token.as_str()),
            ];
            request_token(
                transport,
                &endpoint.shop,
                &endpoint.user_agent,
                Encoding::Json,
                &fields,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiry_uses_the_leeway() {
        let mut token = TokenData::new("token");
        assert!(!token.expires_within(chrono::Duration::days(365)));
        token.expires_at = Some(Utc::now() + chrono::Duration::minutes(1));
        assert!(token.expires_within(chrono::Duration::minutes(5)));
        assert!(!token.expires_within(chrono::Duration::seconds(1)));
    }

    #[test]
    fn a_refresh_token_works_until_it_expires() {
        let mut token = TokenData::new("token");
        assert!(!token.can_refresh());
        token.refresh_token = Some("refresh".to_string());
        assert!(token.can_refresh());
        token.refresh_token_expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        assert!(!token.can_refresh());
    }

    #[test]
    fn debug_hides_tokens_and_secrets() {
        let mut token = TokenData::new("shpat_secret");
        token.refresh_token = Some("shprt_secret".to_string());
        let auth = Auth::expiring_token("id", "client-secret", token);
        let shown = format!("{auth:?}");
        for secret in ["shpat_secret", "shprt_secret", "client-secret"] {
            assert!(!shown.contains(secret), "{shown}");
        }
    }

    #[test]
    fn tokens_round_trip_through_json() {
        let mut token = TokenData::new("shpat");
        token.scope = Some("read_products, write_orders".to_string());
        token.expires_at = Some(Utc::now());
        let json = serde_json::to_string(&token).unwrap();
        assert_eq!(serde_json::from_str::<TokenData>(&json).unwrap(), token);
        assert_eq!(token.scopes(), ["read_products", "write_orders"]);
        // Fields left out of a stored token are read as absent.
        let minimal: TokenData = serde_json::from_str(r#"{"access_token":"shpat"}"#).unwrap();
        assert_eq!(minimal, TokenData::new("shpat"));
    }
}
