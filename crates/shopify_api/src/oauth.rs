//! OAuth and the requests Shopify signs for an app: install links and their
//! callback, session tokens and the token exchange of embedded apps, app proxies.
//!
//! An embedded app gets its tokens from the session token of each request:
//!
//! ```no_run
//! use shopify_api::oauth::{RequestedToken, ShopifyApp};
//!
//! # async fn handle(bearer: &str) -> shopify_api::Result<()> {
//! let app = ShopifyApp::new("client-id", "client-secret");
//!
//! // `Authorization: Bearer …` of a request from the app's frontend.
//! let session = app.verify_session_token(bearer)?;
//! let token = app.exchange_session_token(&session, RequestedToken::ExpiringOffline).await?;
//! // Store `token` (it serializes), then:
//! let shopify = app.client(session.shop(), token)?;
//! # Ok(())
//! # }
//! ```
//!
//! [`verify_session_token`](ShopifyApp::verify_session_token) takes only the
//! tokens of Shopify admin, which a staff member's browser sends. The tokens
//! of checkout and customer account UI extensions, which any buyer gets, are
//! verified by [`verify_extension_session_token`](ShopifyApp::verify_extension_session_token),
//! as an [`ExtensionSessionToken`]: a type of its own, so that one never
//! passes for the other.
//!
//! An app outside Shopify admin installs through a link and a callback:
//!
//! ```no_run
//! use shopify_api::ShopDomain;
//! use shopify_api::oauth::{AuthorizationRequest, ShopifyApp};
//!
//! # async fn run(query_of_the_callback: &str) -> shopify_api::Result<()> {
//! let app = ShopifyApp::new("client-id", "client-secret");
//! let shop = ShopDomain::new("my-shop")?;
//! let link = app.authorization_url(
//!     &shop,
//!     &AuthorizationRequest::new("https://example.com/auth/callback", "a-random-state")
//!         .scopes(["read_products", "write_orders"]),
//! );
//!
//! // On the callback: check the signature, then the state you stored.
//! let callback = app.verify_query(query_of_the_callback)?;
//! assert_eq!(callback.get("state"), Some("a-random-state"));
//! let token = app.exchange_code(&callback).await?;
//! # Ok(())
//! # }
//! ```

use std::sync::Arc;

use base64::Engine;
use base64::alphabet::URL_SAFE;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use chrono::{DateTime, TimeZone, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::Value;
use sha2::Sha256;

use crate::auth::{Encoding, request_token};
use crate::transport::{Transport, default_transport};
use crate::{
    Auth, Error, Result, ShopDomain, Shopify, ShopifyBuilder, TokenData, TokenStore, USER_AGENT,
};

/// How far the `timestamp` of a signed query may be from now, as Shopify's libraries allow.
const QUERY_TOLERANCE_SECONDS: i64 = 90;

/// How far `exp` and `nbf` of a session token may be from now, as Shopify's libraries allow.
const JWT_TOLERANCE_SECONDS: i64 = 10;

/// The parameters of an app proxy request that Shopify sends once.
const PROXY_SINGLE_VALUES: [&str; 4] = ["hmac", "shop", "signature", "timestamp"];

/// base64url, with or without padding.
const BASE64_URL: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new()
        .with_encode_padding(false)
        .with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

fn mac(secret: &str) -> Hmac<Sha256> {
    Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes a key of any length")
}

fn invalid(message: impl Into<String>) -> Error {
    Error::Verification(message.into())
}

/// An app: its client ID and secret, what it signs and verifies, and the
/// tokens it obtains for shops.
#[derive(Clone)]
pub struct ShopifyApp {
    client_id: String,
    client_secret: String,
    transport: Option<Arc<dyn Transport>>,
    token_store: Option<Arc<dyn TokenStore>>,
    user_agent: String,
}

impl std::fmt::Debug for ShopifyApp {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ShopifyApp")
            .field("client_id", &self.client_id)
            .field("token_store", &self.token_store.is_some())
            .finish_non_exhaustive()
    }
}

/// What the install link asks the merchant for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationRequest {
    redirect_uri: String,
    state: String,
    scopes: Vec<String>,
    online: bool,
}

impl AuthorizationRequest {
    /// `redirect_uri` is one of the app's allowed redirection addresses;
    /// `state` a random value you keep for the browser, and compare on the callback.
    pub fn new(redirect_uri: impl Into<String>, state: impl Into<String>) -> Self {
        Self {
            redirect_uri: redirect_uri.into(),
            state: state.into(),
            scopes: Vec::new(),
            online: false,
        }
    }

    /// The access scopes, such as `read_products`.
    pub fn scopes<I: IntoIterator<Item = S>, S: Into<String>>(mut self, scopes: I) -> Self {
        self.scopes = scopes.into_iter().map(Into::into).collect();
        self
    }

    /// An online token, bound to the staff member who installs, instead of
    /// an offline token of the shop.
    pub fn online(mut self) -> Self {
        self.online = true;
        self
    }
}

/// A query string whose signature was verified: an OAuth callback, a launch
/// from Shopify admin, an app proxy request.
///
/// `Debug` shows the names of the parameters and the shop, not the values,
/// which hold an authorization code or a session token.
#[derive(Clone, PartialEq, Eq)]
pub struct SignedQuery {
    params: Vec<(String, String)>,
    shop: Option<ShopDomain>,
}

impl std::fmt::Debug for SignedQuery {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.params.iter().map(|(name, _)| name.as_str()).collect();
        formatter
            .debug_struct("SignedQuery")
            .field("params", &names)
            .field("shop", &self.shop)
            .finish()
    }
}

impl SignedQuery {
    /// The first value of a parameter.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(given, _)| given == name)
            .map(|(_, value)| value.as_str())
    }

    /// Every parameter, as received.
    pub fn params(&self) -> &[(String, String)] {
        &self.params
    }

    /// The `shop` parameter, when it is a shop's domain.
    pub fn shop(&self) -> Option<&ShopDomain> {
        self.shop.as_ref()
    }
}

/// The token an embedded app asks for with a session token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RequestedToken {
    /// A token of the shop that does not expire.
    Offline,
    /// A token of the shop that expires after an hour, with a refresh token.
    ExpiringOffline,
    /// A token bound to the staff member of the session.
    Online,
}

/// A session token of Shopify admin, from [`ShopifyApp::verify_session_token`]:
/// its signature, dates, `dest` and `iss` were verified.
///
/// Only Shopify admin issues it, to a staff member of the shop: it is what
/// opens the admin side of an app. The tokens of checkout and customer account
/// UI extensions, which any buyer gets, are an [`ExtensionSessionToken`].
///
/// `Debug` hides the token itself, which is a bearer.
#[derive(Clone, PartialEq)]
pub struct SessionToken {
    shop: ShopDomain,
    user_id: Option<String>,
    session_id: Option<String>,
    expires_at: DateTime<Utc>,
    claims: Value,
    token: String,
}

impl std::fmt::Debug for SessionToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionToken")
            .field("shop", &self.shop)
            .field("user_id", &self.user_id)
            .field("session_id", &self.session_id)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl SessionToken {
    /// The shop of `dest`.
    pub fn shop(&self) -> &ShopDomain {
        &self.shop
    }

    /// `sub`: the ID of the staff member, such as `42`.
    pub fn user_id(&self) -> Option<&str> {
        self.user_id.as_deref()
    }

    /// The session, `sid`.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// Every claim of the token.
    pub fn claims(&self) -> &Value {
        &self.claims
    }

    /// The token itself, as the frontend sent it.
    pub fn as_str(&self) -> &str {
        &self.token
    }
}

/// A session token of a checkout or customer account UI extension, from
/// [`ShopifyApp::verify_extension_session_token`]: its signature, dates and
/// `dest` were verified.
///
/// Any buyer of the shop gets one, logged in or not: it proves which shop
/// and, through [`customer`](Self::customer), which customer, never a staff
/// member. It is not a [`SessionToken`], and does not exchange for a token.
///
/// `Debug` hides the token itself, which is a bearer.
#[derive(Clone, PartialEq)]
pub struct ExtensionSessionToken {
    shop: ShopDomain,
    customer: Option<String>,
    expires_at: DateTime<Utc>,
    claims: Value,
    token: String,
}

impl std::fmt::Debug for ExtensionSessionToken {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExtensionSessionToken")
            .field("shop", &self.shop)
            .field("customer", &self.customer)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl ExtensionSessionToken {
    /// The shop of `dest`.
    pub fn shop(&self) -> &ShopDomain {
        &self.shop
    }

    /// `sub`: the GID of the logged-in customer, such as
    /// `gid://shopify/Customer/1`. Shopify only sends it when a customer is
    /// logged in and the app may read customers. `None` for an admin token,
    /// whose `sub` is a staff member, not a customer: it stays in
    /// [`claims`](Self::claims).
    pub fn customer(&self) -> Option<&str> {
        self.customer.as_deref()
    }

    pub fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// Every claim of the token.
    pub fn claims(&self) -> &Value {
        &self.claims
    }

    /// The token itself, as the extension sent it.
    pub fn as_str(&self) -> &str {
        &self.token
    }
}

/// The claims of a session token whose signature, dates and audience were verified.
struct Verified {
    claims: Value,
    expires_at: DateTime<Utc>,
    token: String,
}

impl Verified {
    fn text(&self, name: &str) -> Option<String> {
        self.claims
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    fn claim(&self, name: &str) -> Option<&str> {
        self.claims.get(name).and_then(Value::as_str)
    }
}

/// The shop of a host that is a whole `*.myshopify.com` domain.
fn shop_of_host(host: &str) -> Result<ShopDomain> {
    Some(host)
        // A whole domain: a bare handle is not what Shopify writes.
        .filter(|host| host.to_ascii_lowercase().ends_with(".myshopify.com"))
        .and_then(|host| ShopDomain::new(host).ok())
        .ok_or_else(|| invalid("the `dest` of the session token is not a shop"))
}

/// Checks that `iss` is exactly the admin of `shop`.
fn check_admin_issuer(issuer: &str, shop: &ShopDomain) -> Result<()> {
    if issuer == format!("{}/admin", shop.origin()) {
        Ok(())
    } else {
        Err(invalid(
            "the `iss` of the session token is not the admin of its `dest`",
        ))
    }
}

impl ShopifyApp {
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            // Without one, the requests that need it fail with `NoTransport`.
            transport: default_transport().ok(),
            token_store: None,
            user_agent: USER_AGENT.to_string(),
        }
    }

    /// Sends the token requests, and the requests of the clients it builds,
    /// through `transport`.
    pub fn with_transport(mut self, transport: impl Transport) -> Self {
        self.transport = Some(Arc::new(transport));
        self
    }

    /// Saves the tokens that the clients it builds obtain or renew: the
    /// refresh token of an expiring token changes at each renewal. The store
    /// is told the shop of each token.
    pub fn with_token_store(mut self, store: impl TokenStore) -> Self {
        self.token_store = Some(Arc::new(store));
        self
    }

    pub fn with_user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = user_agent.into();
        self
    }

    pub fn client_id(&self) -> &str {
        &self.client_id
    }

    fn transport(&self) -> Result<&dyn Transport> {
        self.transport.as_deref().ok_or(Error::NoTransport)
    }

    /// The secret to verify with. An empty one would verify what anyone signs.
    fn secret(&self) -> Result<&str> {
        if self.client_secret.is_empty() {
            return Err(invalid(
                "the client secret of the app is empty: nothing can be verified with it",
            ));
        }
        Ok(&self.client_secret)
    }

    // ─── Clients ─────────────────────────────────────────────────────

    /// A builder of a client of `shop` that uses the app's transport and
    /// token store.
    pub fn builder(&self, shop: &ShopDomain) -> ShopifyBuilder {
        let mut builder = Shopify::builder(shop).user_agent(&self.user_agent);
        if let Some(transport) = &self.transport {
            builder = builder.shared_transport(transport.clone());
        }
        if let Some(store) = &self.token_store {
            builder = builder.shared_token_store(store.clone());
        }
        builder
    }

    /// A client of `shop` with a token the app obtained: renewed with its
    /// refresh token when it has one, used as it is otherwise. The renewed
    /// tokens go to the app's [token store](Self::with_token_store).
    pub fn client(&self, shop: &ShopDomain, token: TokenData) -> Result<Shopify> {
        let auth = if token.refresh_token.is_some() {
            Auth::expiring_token(&self.client_id, &self.client_secret, token)
        } else {
            Auth::token(token)
        };
        self.builder(shop).auth(auth).build()
    }

    /// A client of `shop` that gets its tokens with the client credentials
    /// grant: for a shop of the organization that owns the app.
    pub fn client_credentials_client(&self, shop: &ShopDomain) -> Result<Shopify> {
        self.builder(shop)
            .client_credentials(&self.client_id, &self.client_secret)
            .build()
    }

    // ─── Install link and callback ───────────────────────────────────

    /// The address that asks the merchant to install the app.
    pub fn authorization_url(&self, shop: &ShopDomain, request: &AuthorizationRequest) -> String {
        let scope = request.scopes.join(",");
        let mut params = vec![
            ("client_id", self.client_id.as_str()),
            ("scope", scope.as_str()),
            ("redirect_uri", request.redirect_uri.as_str()),
            ("state", request.state.as_str()),
        ];
        if request.online {
            params.push(("grant_options[]", "per-user"));
        }
        let query = serde_urlencoded::to_string(&params)
            .unwrap_or_default()
            .replace('+', "%20");
        format!("https://{shop}/admin/oauth/authorize?{query}")
    }

    /// Exchanges the `code` of a verified callback for a token.
    pub async fn exchange_code(&self, callback: &SignedQuery) -> Result<TokenData> {
        self.exchange_code_with(callback, false).await
    }

    /// [`ShopifyApp::exchange_code`], asking for an expiring offline token
    /// with a refresh token.
    pub async fn exchange_code_expiring(&self, callback: &SignedQuery) -> Result<TokenData> {
        self.exchange_code_with(callback, true).await
    }

    async fn exchange_code_with(
        &self,
        callback: &SignedQuery,
        expiring: bool,
    ) -> Result<TokenData> {
        let shop = callback
            .shop()
            .ok_or_else(|| invalid("the callback has no valid `shop`"))?;
        let code = callback
            .get("code")
            .filter(|code| !code.is_empty())
            .ok_or_else(|| invalid("the callback has no `code`"))?;
        let mut fields = vec![
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            ("code", code),
        ];
        if expiring {
            fields.push(("expiring", "1"));
        }
        request_token(
            self.transport()?,
            shop,
            &self.user_agent,
            Encoding::Json,
            &fields,
        )
        .await
    }

    // ─── Signed queries ──────────────────────────────────────────────

    /// Verifies the `hmac` of a query string from Shopify admin: the OAuth
    /// callback, a launch of the app, an admin link. Its `timestamp` must be
    /// within 90 seconds of now.
    ///
    /// `query` is the raw query string, with or without its `?`. The message
    /// signed is the one Shopify's JavaScript library computes: the
    /// parameters but `hmac` and `signature`, sorted by name, form-encoded,
    /// the values of a repeated one (`ids[]`) joined with commas.
    pub fn verify_query(&self, query: &str) -> Result<SignedQuery> {
        self.verify_query_at(query, Utc::now())
    }

    fn verify_query_at(&self, query: &str, now: DateTime<Utc>) -> Result<SignedQuery> {
        let params = parse_query(query)?;
        let signature =
            single(&params, "hmac")?.ok_or_else(|| invalid("the query has no `hmac`"))?;
        let expected = hex::decode(signature)
            .map_err(|_| invalid("the `hmac` of the query is not hexadecimal"))?;
        let mut mac = mac(self.secret()?);
        mac.update(admin_message(&params).as_bytes());
        mac.verify_slice(&expected)
            .map_err(|_| invalid("the signature of the query does not match"))?;
        // Only once the signature holds: nothing unsigned reaches the clock.
        check_timestamp(&params, now)?;
        Ok(signed(params))
    }

    /// The `hmac` Shopify signs a query of Shopify admin with: for tests.
    #[cfg(any(test, feature = "testing"))]
    #[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
    pub fn sign_query(&self, query: &str) -> Result<String> {
        let params = parse_query(query)?;
        let mut mac = mac(&self.client_secret);
        mac.update(admin_message(&params).as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    /// Verifies the `signature` of an app proxy request. Its `timestamp`
    /// must be within 90 seconds of now.
    ///
    /// The message signed is the one Shopify computes: for each parameter but
    /// `signature`, the string `name=value` (decoded, the values of a repeated
    /// one joined with commas), these strings sorted and joined with nothing.
    /// The whole strings are sorted, not the names: `page2=x` comes before
    /// `page=1`.
    pub fn verify_app_proxy(&self, query: &str) -> Result<SignedQuery> {
        self.verify_app_proxy_at(query, Utc::now())
    }

    fn verify_app_proxy_at(&self, query: &str, now: DateTime<Utc>) -> Result<SignedQuery> {
        let params = parse_query(query)?;
        for name in PROXY_SINGLE_VALUES {
            single(&params, name)?;
        }
        let signature =
            single(&params, "signature")?.ok_or_else(|| invalid("the query has no `signature`"))?;
        let expected = hex::decode(signature)
            .map_err(|_| invalid("the `signature` of the query is not hexadecimal"))?;
        let mut mac = mac(self.secret()?);
        mac.update(proxy_message(&params).as_bytes());
        mac.verify_slice(&expected)
            .map_err(|_| invalid("the signature of the app proxy request does not match"))?;
        // Only once the signature holds: nothing unsigned reaches the clock.
        check_timestamp(&params, now)?;
        Ok(signed(params))
    }

    /// The `signature` Shopify signs an app proxy request with: for tests.
    #[cfg(any(test, feature = "testing"))]
    #[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
    pub fn sign_app_proxy(&self, query: &str) -> Result<String> {
        let params = parse_query(query)?;
        let mut mac = mac(&self.client_secret);
        mac.update(proxy_message(&params).as_bytes());
        Ok(hex::encode(mac.finalize().into_bytes()))
    }

    // ─── Session tokens ──────────────────────────────────────────────

    /// Verifies a session token of Shopify admin (the bearer of the requests
    /// of the app's embedded frontend): signed by the app's secret with HS256,
    /// issued for the app, within its dates, with a `dest` of exactly
    /// `https://{shop}.myshopify.com` and an `iss` of exactly
    /// `https://{shop}.myshopify.com/admin`.
    ///
    /// Only Shopify admin issues such a token, to a staff member: trust it to
    /// open the admin side of the app, such as loading the shop's stored
    /// offline token. The tokens of checkout and customer account UI
    /// extensions, which any buyer gets, are refused here: verify them with
    /// [`verify_extension_session_token`](Self::verify_extension_session_token).
    pub fn verify_session_token(&self, token: &str) -> Result<SessionToken> {
        self.verify_session_token_at(token, Utc::now())
    }

    fn verify_session_token_at(&self, token: &str, now: DateTime<Utc>) -> Result<SessionToken> {
        let verified = self.verify_jwt(token, now)?;
        let destination = verified
            .claim("dest")
            .ok_or_else(|| invalid("the session token has no `dest`"))?;
        let shop = destination
            .strip_prefix("https://")
            .ok_or_else(|| invalid("the `dest` of the session token is not the admin of a shop"))
            .and_then(shop_of_host)?;
        if destination != shop.origin() {
            return Err(invalid("the `dest` of the session token is not a shop"));
        }
        let issuer = verified.claim("iss").ok_or_else(|| {
            invalid("the session token has no `iss`: it is not from Shopify admin")
        })?;
        check_admin_issuer(issuer, &shop)?;
        Ok(SessionToken {
            user_id: verified.text("sub"),
            session_id: verified.text("sid"),
            shop,
            expires_at: verified.expires_at,
            claims: verified.claims,
            token: verified.token,
        })
    }

    /// Verifies the session token of a checkout or customer account UI
    /// extension (`shopify.sessionToken.get()`): signed by the app's secret with
    /// HS256, issued for the app, within its dates, with a `dest` that is a
    /// `*.myshopify.com` domain (Shopify writes it without `https://`). An
    /// `iss`, when there is one, must be exactly `https://{shop}.myshopify.com/admin`.
    ///
    /// Any buyer of the shop can get such a token: it proves the shop and,
    /// when one is logged in, the customer, never a staff member. It is a type
    /// of its own so that it cannot reach what a [`SessionToken`] opens.
    pub fn verify_extension_session_token(&self, token: &str) -> Result<ExtensionSessionToken> {
        self.verify_extension_session_token_at(token, Utc::now())
    }

    fn verify_extension_session_token_at(
        &self,
        token: &str,
        now: DateTime<Utc>,
    ) -> Result<ExtensionSessionToken> {
        let verified = self.verify_jwt(token, now)?;
        let destination = verified
            .claim("dest")
            .ok_or_else(|| invalid("the session token has no `dest`"))?;
        let shop = shop_of_host(destination.strip_prefix("https://").unwrap_or(destination))?;
        if let Some(issuer) = verified.claim("iss") {
            check_admin_issuer(issuer, &shop)?;
        }
        Ok(ExtensionSessionToken {
            // Only a customer's GID: the `sub` of an admin token is a staff member.
            customer: verified
                .text("sub")
                .filter(|sub| sub.starts_with("gid://shopify/Customer/")),
            shop,
            expires_at: verified.expires_at,
            claims: verified.claims,
            token: verified.token,
        })
    }

    /// The signature, dates and audience of a session token, the same for every kind.
    fn verify_jwt(&self, token: &str, now: DateTime<Utc>) -> Result<Verified> {
        let token = token.trim();
        let token = token.strip_prefix("Bearer ").unwrap_or(token);
        let mut parts = token.split('.');
        let (Some(header), Some(payload), Some(signature), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(invalid("the session token is not a JWT"));
        };
        let decode = |part: &str| {
            BASE64_URL
                .decode(part)
                .map_err(|_| invalid("the session token is not base64url"))
        };
        let header: Value = serde_json::from_slice(&decode(header)?)
            .map_err(|_| invalid("the session token has an invalid header"))?;
        if header.get("alg").and_then(Value::as_str) != Some("HS256") {
            return Err(invalid("the session token is not signed with HS256"));
        }
        let mut mac = mac(self.secret()?);
        // What is signed: the header and the payload, with the dot between them.
        mac.update(&token.as_bytes()[..token.len() - signature.len() - 1]);
        mac.verify_slice(&decode(signature)?)
            .map_err(|_| invalid("the signature of the session token does not match"))?;

        let claims: Value = serde_json::from_slice(&decode(payload)?)
            .map_err(|_| invalid("the session token has invalid claims"))?;
        let time = |name: &str| claims.get(name).and_then(Value::as_i64);
        let now = now.timestamp();
        let expires = time("exp").ok_or_else(|| invalid("the session token has no `exp`"))?;
        if now > expires.saturating_add(JWT_TOLERANCE_SECONDS) {
            return Err(invalid("the session token has expired"));
        }
        if time("nbf")
            .is_some_and(|not_before| now < not_before.saturating_sub(JWT_TOLERANCE_SECONDS))
        {
            return Err(invalid("the session token is not valid yet"));
        }
        let audience_matches = match claims.get("aud") {
            Some(Value::String(audience)) => audience == &self.client_id,
            Some(Value::Array(audiences)) => audiences
                .iter()
                .any(|audience| audience.as_str() == Some(&self.client_id)),
            _ => false,
        };
        if !audience_matches {
            return Err(invalid("the session token was issued for another app"));
        }
        Ok(Verified {
            expires_at: Utc
                .timestamp_opt(expires, 0)
                .single()
                .ok_or_else(|| invalid("the `exp` of the session token is out of range"))?,
            claims,
            token: token.to_string(),
        })
    }

    /// A session token signed with the app's secret, as Shopify makes them:
    /// for tests. `claims` should hold `iss`, `dest`, `aud`, `exp` and `nbf`.
    #[cfg(any(test, feature = "testing"))]
    #[cfg_attr(docsrs, doc(cfg(feature = "testing")))]
    pub fn sign_session_token(&self, claims: &Value) -> String {
        let header = BASE64_URL.encode(br#"{"alg":"HS256","typ":"JWT"}"#);
        let payload = BASE64_URL.encode(serde_json::to_vec(claims).unwrap_or_default());
        let mut mac = mac(&self.client_secret);
        mac.update(format!("{header}.{payload}").as_bytes());
        let signature = BASE64_URL.encode(mac.finalize().into_bytes());
        format!("{header}.{payload}.{signature}")
    }

    /// Exchanges a verified session token for an access token of its shop.
    pub async fn exchange_session_token(
        &self,
        session: &SessionToken,
        requested: RequestedToken,
    ) -> Result<TokenData> {
        let (requested_type, expiring) = match requested {
            RequestedToken::Offline => (
                "urn:shopify:params:oauth:token-type:offline-access-token",
                "0",
            ),
            RequestedToken::ExpiringOffline => (
                "urn:shopify:params:oauth:token-type:offline-access-token",
                "1",
            ),
            RequestedToken::Online => (
                "urn:shopify:params:oauth:token-type:online-access-token",
                "0",
            ),
        };
        let fields = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.as_str()),
            (
                "grant_type",
                "urn:ietf:params:oauth:grant-type:token-exchange",
            ),
            ("subject_token", session.as_str()),
            (
                "subject_token_type",
                "urn:ietf:params:oauth:token-type:id_token",
            ),
            ("requested_token_type", requested_type),
            ("expiring", expiring),
        ];
        request_token(
            self.transport()?,
            session.shop(),
            &self.user_agent,
            Encoding::Json,
            &fields,
        )
        .await
    }
}

fn parse_query(query: &str) -> Result<Vec<(String, String)>> {
    let query = query.trim();
    let query = query.strip_prefix('?').unwrap_or(query);
    serde_urlencoded::from_str(query).map_err(|_| invalid("the query string does not parse"))
}

/// The value of a parameter that must not appear twice.
fn single<'a>(params: &'a [(String, String)], name: &str) -> Result<Option<&'a str>> {
    let mut values = params.iter().filter(|(given, _)| given == name);
    let first = values.next().map(|(_, value)| value.as_str());
    if values.next().is_some() {
        return Err(invalid(format!("the query has `{name}` more than once")));
    }
    Ok(first)
}

fn check_timestamp(params: &[(String, String)], now: DateTime<Utc>) -> Result<()> {
    let timestamp: i64 = single(params, "timestamp")?
        .and_then(|timestamp| timestamp.parse().ok())
        .ok_or_else(|| invalid("the query has no valid `timestamp`"))?;
    // `abs_diff`: any `i64` timestamp, however far, is a distance, not an overflow.
    if now.timestamp().abs_diff(timestamp) > QUERY_TOLERANCE_SECONDS.unsigned_abs() {
        return Err(invalid(
            "the `timestamp` of the query is more than 90 seconds away",
        ));
    }
    Ok(())
}

/// The parameters signed: every one but the signature, the values of a
/// repeated one joined with commas, in the order they first appear.
fn grouped_params(params: &[(String, String)]) -> Vec<(String, String)> {
    let mut grouped: Vec<(String, String)> = Vec::new();
    for (name, value) in params {
        if name == "hmac" || name == "signature" {
            continue;
        }
        match grouped.iter_mut().find(|(given, _)| given == name) {
            Some((_, joined)) => {
                joined.push(',');
                joined.push_str(value);
            }
            None => grouped.push((name.clone(), value.clone())),
        }
    }
    grouped
}

/// What Shopify admin signs: `name=value` pairs sorted by name,
/// form-encoded with `%20` for spaces, joined with `&`.
fn admin_message(params: &[(String, String)]) -> String {
    let mut grouped = grouped_params(params);
    grouped.sort_by(|left, right| left.0.cmp(&right.0));
    serde_urlencoded::to_string(grouped)
        .unwrap_or_default()
        .replace('+', "%20")
}

/// What an app proxy signs: the `name=value` strings, as received, sorted
/// as whole strings and joined with nothing.
///
/// Not sorted by name: Shopify sorts the strings, so `page2=x` comes before
/// `page=1` (`2` sorts before `=`), where a sort by name puts `page` first.
fn proxy_message(params: &[(String, String)]) -> String {
    let mut pairs: Vec<String> = grouped_params(params)
        .into_iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    pairs.sort_unstable();
    pairs.concat()
}

fn signed(params: Vec<(String, String)>) -> SignedQuery {
    let shop = params
        .iter()
        .find(|(name, _)| name == "shop")
        .and_then(|(_, shop)| ShopDomain::new(shop).ok());
    SignedQuery { params, shop }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app() -> ShopifyApp {
        ShopifyApp::new("client-id", "hush")
    }

    fn at(timestamp: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(timestamp, 0).unwrap()
    }

    #[test]
    fn verifies_the_example_of_shopify_documentation() {
        // The OAuth callback of Shopify's documentation, signed with "hush".
        let query = "code=0907a61c0c8d55e99db179b68161bc00&hmac=700e2dadb827fcc8609e9d5ce208b2e9cdaab9df07390d2cbca10d7c328fc4bf&shop=some-shop.myshopify.com&state=0.6784241404160823&timestamp=1337178173";
        let callback = app().verify_query_at(query, at(1337178173 + 30)).unwrap();
        assert_eq!(
            callback.get("code"),
            Some("0907a61c0c8d55e99db179b68161bc00")
        );
        assert_eq!(callback.shop().unwrap().as_str(), "some-shop.myshopify.com");

        let error = app()
            .verify_query_at(query, at(1337178173 + 91))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "verification failed: the `timestamp` of the query is more than 90 seconds away"
        );
        let tampered = query.replace("some-shop", "other-shop");
        assert_eq!(
            app()
                .verify_query_at(&tampered, at(1337178173))
                .unwrap_err()
                .to_string(),
            "verification failed: the signature of the query does not match"
        );
    }

    #[test]
    fn extreme_timestamps_are_rejected_without_panicking() {
        for timestamp in [i64::MIN, i64::MAX] {
            let query = format!("timestamp={timestamp}&hmac=00&signature=00");
            for result in [
                app().verify_query_at(&query, at(1_700_000_000)),
                app().verify_app_proxy_at(&query, at(1_700_000_000)),
            ] {
                assert!(matches!(result, Err(Error::Verification(_))), "{result:?}");
            }
            // Signed, so only the timestamp fails.
            let unsigned = format!("shop=a.myshopify.com&timestamp={timestamp}");
            let hmac = app().sign_query(&unsigned).unwrap();
            let error = app()
                .verify_query_at(&format!("{unsigned}&hmac={hmac}"), at(1_700_000_000))
                .unwrap_err();
            assert!(error.to_string().contains("90 seconds"), "{error}");
            let signature = app().sign_app_proxy(&unsigned).unwrap();
            let error = app()
                .verify_app_proxy_at(
                    &format!("{unsigned}&signature={signature}"),
                    at(1_700_000_000),
                )
                .unwrap_err();
            assert!(error.to_string().contains("90 seconds"), "{error}");
        }
    }

    #[test]
    fn extreme_session_token_times_do_not_panic() {
        let now = 1_700_000_000;
        for (exp, nbf) in [(i64::MAX, now), (now + 60, i64::MIN), (i64::MIN, now)] {
            let token = app().sign_session_token(&json!({
                "iss": "https://a.myshopify.com/admin",
                "dest": "https://a.myshopify.com",
                "aud": "client-id",
                "exp": exp,
                "nbf": nbf,
            }));
            let _ = app().verify_session_token_at(&token, at(now));
        }
    }

    #[test]
    fn signs_queries_as_shopify_admin_does() {
        // Spaces as %20, repeated values joined, names sorted, the signature left out.
        let params =
            parse_query("shop=a.myshopify.com&ids[]=2&ids[]=3&timestamp=10&note=a b&hmac=x")
                .unwrap();
        assert_eq!(
            admin_message(&params),
            "ids%5B%5D=2%2C3&note=a%20b&shop=a.myshopify.com&timestamp=10"
        );
        let query = format!(
            "timestamp={}&shop=a.myshopify.com&host=YWRtaW4",
            Utc::now().timestamp()
        );
        let signature = app().sign_query(&query).unwrap();
        assert!(
            app()
                .verify_query(&format!("?{query}&hmac={signature}"))
                .is_ok()
        );
        assert_eq!(
            app().verify_query(&query).unwrap_err().to_string(),
            "verification failed: the query has no `hmac`"
        );
    }

    #[test]
    fn verifies_app_proxy_requests() {
        let now = Utc::now().timestamp();
        let query =
            format!("shop=a.myshopify.com&path_prefix=%2Fapps%2Fx&timestamp={now}&extra=1&extra=2");
        assert_eq!(
            proxy_message(&parse_query(&query).unwrap()),
            format!("extra=1,2path_prefix=/apps/xshop=a.myshopify.comtimestamp={now}")
        );
        let signature = app().sign_app_proxy(&query).unwrap();
        let verified = app()
            .verify_app_proxy(&format!("{query}&signature={signature}"))
            .unwrap();
        assert_eq!(verified.get("path_prefix"), Some("/apps/x"));
        assert!(
            app()
                .verify_app_proxy(&format!(
                    "{query}&signature={signature}&shop=b.myshopify.com"
                ))
                .is_err()
        );
    }

    #[test]
    fn app_proxy_signs_the_sorted_name_value_strings() {
        // Shopify sorts `name=value` whole: `page2=x` before `page=1`
        // ('2' < '='), `filter.v.price.gte=10` before `filter=red` ('.' < '=').
        // The signature below is HMAC-SHA256("hush", message), computed
        // outside the crate (`openssl dgst -sha256 -hmac hush`).
        let query = "page=1&page2=x&filter=red&filter.v.price.gte=10\
                     &shop=a.myshopify.com&path_prefix=%2Fapps%2Fx&timestamp=1700000000";
        let message = "filter.v.price.gte=10filter=redpage2=xpage=1\
                       path_prefix=/apps/xshop=a.myshopify.comtimestamp=1700000000";
        let signature = "e640778c56c80f8a92904d2b740665f8dd3b3d2d0f30095aa325bf576074f0f9";
        assert_eq!(proxy_message(&parse_query(query).unwrap()), message);
        assert_eq!(app().sign_app_proxy(query).unwrap(), signature);
        let verified = app()
            .verify_app_proxy_at(&format!("{query}&signature={signature}"), at(1_700_000_000))
            .unwrap();
        assert_eq!(verified.get("page2"), Some("x"));

        // `-` sorts before `=` as well.
        assert_eq!(
            proxy_message(&parse_query("filter=red&filter-x=1").unwrap()),
            "filter-x=1filter=red"
        );
        assert_eq!(
            app().sign_app_proxy("filter=red&filter-x=1").unwrap(),
            "31d2d42ad3281aaaf8ca1a9b40f2b21387aee62103e43c3b1564938817b26205"
        );
    }

    fn claims(now: i64) -> Value {
        json!({
            "iss": "https://my-shop.myshopify.com/admin",
            "dest": "https://my-shop.myshopify.com",
            "aud": "client-id",
            "sub": "42",
            "exp": now + 60,
            "nbf": now - 1,
            "iat": now - 1,
            "jti": "f8912129-1af6-4cad-9ca3-76b0f7621087",
            "sid": "aaea182f2732d44c23057c0fea584021a4485b2bd25d3eb7fd349313ad24c685",
        })
    }

    #[test]
    fn verifies_session_tokens() {
        let now = 1_800_000_000;
        let token = app().sign_session_token(&claims(now));
        let session = app()
            .verify_session_token_at(&format!("Bearer {token}"), at(now))
            .unwrap();
        assert_eq!(session.shop().as_str(), "my-shop.myshopify.com");
        assert_eq!(session.user_id(), Some("42"));
        assert_eq!(session.session_id(), claims(now)["sid"].as_str());
        assert_eq!(session.expires_at(), at(now + 60));
        assert_eq!(session.as_str(), token);

        let refusal = |token: &str, time: i64| {
            app()
                .verify_session_token_at(token, at(time))
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            refusal(&token, now + 71),
            "verification failed: the session token has expired"
        );
        assert_eq!(
            refusal(&token, now - 12),
            "verification failed: the session token is not valid yet"
        );
        let other_app = ShopifyApp::new("other", "hush").sign_session_token(&claims(now));
        assert_eq!(
            ShopifyApp::new("client-id", "other")
                .verify_session_token_at(&other_app, at(now))
                .unwrap_err()
                .to_string(),
            "verification failed: the signature of the session token does not match"
        );
        let mut audience = claims(now);
        audience["aud"] = json!("other");
        assert_eq!(
            refusal(&app().sign_session_token(&audience), now),
            "verification failed: the session token was issued for another app"
        );
        let mut foreign = claims(now);
        foreign["dest"] = json!("https://evil.example.com");
        assert_eq!(
            refusal(&app().sign_session_token(&foreign), now),
            "verification failed: the `dest` of the session token is not a shop"
        );
        let mut mixed = claims(now);
        mixed["iss"] = json!("https://other-shop.myshopify.com/admin");
        assert_eq!(
            refusal(&app().sign_session_token(&mixed), now),
            "verification failed: the `iss` of the session token is not the admin of its `dest`"
        );
        // A token whose header claims another algorithm is refused before its signature.
        let unsigned = format!(
            "{}.{}.",
            BASE64_URL.encode(br#"{"alg":"none"}"#),
            BASE64_URL.encode(b"{}")
        );
        assert_eq!(
            refusal(&unsigned, now),
            "verification failed: the session token is not signed with HS256"
        );
        assert_eq!(
            refusal("a.b", now),
            "verification failed: the session token is not a JWT"
        );
    }

    #[test]
    fn an_admin_session_token_has_exactly_the_admin_issuer_and_destination() {
        let now = 1_800_000_000;
        let refusal = |claims: &Value| {
            app()
                .verify_session_token_at(&app().sign_session_token(claims), at(now))
                .unwrap_err()
                .to_string()
        };
        let not_admin =
            "verification failed: the `iss` of the session token is not the admin of its `dest`";
        // `iss` is compared whole: a host that only starts with the shop's is another host.
        for issuer in [
            "https://my-shop.myshopify.com.evil.example/admin",
            "https://my-shop.myshopify.com",
            "https://my-shop.myshopify.com/admin/",
            "https://my-shop.myshopify.com/adminx",
            "http://my-shop.myshopify.com/admin",
        ] {
            let mut token = claims(now);
            token["iss"] = json!(issuer);
            assert_eq!(refusal(&token), not_admin, "{issuer}");
        }
        let mut no_issuer = claims(now);
        no_issuer.as_object_mut().unwrap().remove("iss");
        assert_eq!(
            refusal(&no_issuer),
            "verification failed: the session token has no `iss`: it is not from Shopify admin"
        );
        let mut bare = claims(now);
        bare["dest"] = json!("my-shop.myshopify.com");
        assert_eq!(
            refusal(&bare),
            "verification failed: the `dest` of the session token is not the admin of a shop"
        );
        for destination in [
            "https://my-shop.myshopify.com/",
            "https://my-shop.myshopify.com/admin",
        ] {
            let mut token = claims(now);
            token["dest"] = json!(destination);
            assert!(
                app()
                    .verify_session_token_at(&app().sign_session_token(&token), at(now))
                    .is_err(),
                "{destination}"
            );
        }
    }

    /// The payload of Shopify's documentation of the Session Token API of
    /// checkout and customer account UI extensions: no `iss`, no `sid`, a
    /// `dest` without `https://`, and `sub` only for a logged-in customer.
    fn extension_claims(now: i64) -> Value {
        json!({
            "dest": "store-name.myshopify.com",
            "aud": "client-id",
            "exp": now + 300,
            "nbf": now,
            "iat": now,
            "jti": "6c992878-dbaf-48d1-bb9d-6d9b59814fd1",
            "sub": "gid://shopify/Customer/1",
        })
    }

    #[test]
    fn verifies_extension_session_tokens() {
        let now = 1_800_000_000;
        let verify = |claims: &Value| {
            app().verify_extension_session_token_at(&app().sign_session_token(claims), at(now))
        };
        let session = verify(&extension_claims(now)).unwrap();
        assert_eq!(session.shop().as_str(), "store-name.myshopify.com");
        assert_eq!(session.customer(), Some("gid://shopify/Customer/1"));
        assert_eq!(session.expires_at(), at(now + 300));
        assert_eq!(
            session.claims()["jti"],
            "6c992878-dbaf-48d1-bb9d-6d9b59814fd1"
        );

        // An anonymous buyer: no `sub`.
        let mut anonymous = extension_claims(now);
        anonymous.as_object_mut().unwrap().remove("sub");
        assert_eq!(verify(&anonymous).unwrap().customer(), None);
        // `dest` with `https://`, and an `iss` that is the admin of the same shop.
        let mut admin_shaped = extension_claims(now);
        admin_shaped["dest"] = json!("https://store-name.myshopify.com");
        admin_shaped["iss"] = json!("https://store-name.myshopify.com/admin");
        assert!(verify(&admin_shaped).is_ok());
        // An admin token verifies here too: it proves its shop as well, but
        // its staff member is not a customer.
        let admin = verify(&claims(now)).unwrap();
        assert_eq!(admin.shop().as_str(), "my-shop.myshopify.com");
        assert_eq!(admin.customer(), None);
        assert_eq!(admin.claims()["sub"], claims(now)["sub"]);

        // An `iss` is compared whole here as well.
        let mut prefixed = extension_claims(now);
        prefixed["iss"] = json!("https://store-name.myshopify.com.evil.example/admin");
        assert_eq!(
            verify(&prefixed).unwrap_err().to_string(),
            "verification failed: the `iss` of the session token is not the admin of its `dest`"
        );
        // A bare handle, or another host, is still not a shop.
        for destination in ["store-name", "store-name.example.com", "https://store-name"] {
            let mut token = extension_claims(now);
            token["dest"] = json!(destination);
            assert!(verify(&token).is_err(), "{destination}");
        }
        // The checks every session token gets.
        let mut other_app = extension_claims(now);
        other_app["aud"] = json!("other");
        assert!(verify(&other_app).is_err());
        assert!(
            app()
                .verify_extension_session_token_at(
                    &app().sign_session_token(&extension_claims(now)),
                    at(now + 311)
                )
                .is_err()
        );
        let foreign =
            ShopifyApp::new("client-id", "other").sign_session_token(&extension_claims(now));
        assert!(
            app()
                .verify_extension_session_token_at(&foreign, at(now))
                .is_err()
        );
    }

    #[test]
    fn an_extension_session_token_is_not_an_admin_session() {
        let now = 1_800_000_000;
        let mut anonymous = extension_claims(now);
        anonymous.as_object_mut().unwrap().remove("sub");
        for token in [extension_claims(now), anonymous] {
            let token = app().sign_session_token(&token);
            assert!(app().verify_session_token_at(&token, at(now)).is_err());
            assert!(
                app()
                    .verify_extension_session_token_at(&token, at(now))
                    .is_ok()
            );
        }
    }

    #[test]
    fn an_empty_secret_verifies_nothing() {
        let unset = ShopifyApp::new("client-id", "");
        let now = Utc::now().timestamp();
        let query = format!("shop=a.myshopify.com&timestamp={now}");
        let signature = unset.sign_query(&query).unwrap();
        assert_eq!(
            unset
                .verify_query(&format!("{query}&hmac={signature}"))
                .unwrap_err()
                .to_string(),
            "verification failed: the client secret of the app is empty: nothing can be verified with it"
        );
        let token = unset.sign_session_token(&claims(now));
        assert!(unset.verify_session_token(&token).is_err());
        let signature = unset.sign_app_proxy(&query).unwrap();
        assert!(
            unset
                .verify_app_proxy(&format!("{query}&signature={signature}"))
                .is_err()
        );
    }

    #[test]
    fn debug_hides_codes_and_tokens() {
        let now = Utc::now().timestamp();
        let query = format!("code=secret-code&shop=a.myshopify.com&timestamp={now}");
        let signature = app().sign_query(&query).unwrap();
        let callback = app()
            .verify_query(&format!("{query}&hmac={signature}"))
            .unwrap();
        let shown = format!("{callback:?}");
        assert!(!shown.contains("secret-code"), "{shown}");
        assert!(shown.contains("\"code\""), "{shown}");

        let token = app().sign_session_token(&claims(now));
        let session = app().verify_session_token(&token).unwrap();
        assert!(!format!("{session:?}").contains(&token));
        let extension = app().sign_session_token(&extension_claims(now));
        let session = app().verify_extension_session_token(&extension).unwrap();
        assert!(!format!("{session:?}").contains(&extension));
    }

    #[test]
    fn builds_install_links() {
        let shop = ShopDomain::new("my-shop").unwrap();
        let link = app().authorization_url(
            &shop,
            &AuthorizationRequest::new("https://example.com/auth/callback", "nonce 1")
                .scopes(["read_products", "write_orders"])
                .online(),
        );
        assert_eq!(
            link,
            "https://my-shop.myshopify.com/admin/oauth/authorize?client_id=client-id&scope=read_products%2Cwrite_orders\
             &redirect_uri=https%3A%2F%2Fexample.com%2Fauth%2Fcallback&state=nonce%201&grant_options%5B%5D=per-user"
        );
    }
}
