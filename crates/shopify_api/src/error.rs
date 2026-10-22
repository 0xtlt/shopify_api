//! The error of every fallible call of this crate.

use std::fmt;

use crate::graphql::{Cost, GraphqlError, UserError};
use crate::transport::{HttpResponse, TransportError};

/// `Result<T, shopify_api::Error>`.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What went wrong, with what Shopify said about it.
///
/// # What an error shows
///
/// What an error displays (`Display`, `Debug`, `{:#?}`) and what its
/// [`source`](std::error::Error::source) chain displays is written by this
/// crate. Text the crate does not control is not put in it, whatever it
/// looks like:
///
/// - no body of an answer: [`Error::Http`] shows the status, the address,
///   the kind of content and the length of the body;
/// - no value of an answer, no map key, no message of serde:
///   [`Error::Decode`] says what kind of mismatch it is (`invalid type:
///   string, expected u64`, ``missing field `id` ``, `unknown variant`, a
///   JSON syntax error), where (the line and column of a text), and the path to
///   it, made of the names of the fields and variants the Rust type declares
///   and of indexes; a map key, or a name the type does not declare, is
///   `[key]`;
/// - nothing of what a `Serialize` implementation wrote: [`Error::Variables`]
///   says which variable does not serialize, and why in this crate's words;
/// - of an address, the scheme, the host and the path only: no user, no
///   password, no query string (a signature), no fragment;
/// - of the token endpoint's refusal, its status and its standard OAuth error
///   code;
/// - no shop domain or API version that does not validate: a token given in
///   their place is not shown.
///
/// - of the partial `data` of a GraphQL error, and of its `extensions` but
///   `code`, how many entries there are: no name, no value;
/// - of a bulk operation, its ID when it is a `gid://` of Shopify, a status
///   or error code this crate knows, and which count does not read as a
///   number, not the count;
/// - the name of a variable when it is a GraphQL name, and the names of the
///   fields, variants and types your Rust types declare.
///
/// What is shown as is, because this crate does not write it: the
/// `message`, `path`, `locations` and `extensions.code` of Shopify's
/// `errors`, and the `field`, `message` and `code` of its `userErrors`, which
/// Shopify writes for people (a message may quote a part of a variable that
/// does not validate); the error of your
/// [`Transport`](crate::transport::Transport) (the default transport's leaves
/// the address out) and of your [`TokenStore`](crate::TokenStore), and the
/// `source` chain of either.
///
/// # What stays available
///
/// The text that is not shown stays available to code that asks for it,
/// through accessors that say so: [`Error::raw_http_body`],
/// [`Error::sensitive_detail`] (serde's message, a `Serialize`
/// implementation's, the token endpoint's answer), [`GraphqlErrors::data`],
/// [`GraphqlError::extensions`], and the fields of the variants.
/// They may hold secrets: do not log them.
#[non_exhaustive]
pub enum Error {
    /// Not a shop's handle or `myshopify.com` domain. The text is the one
    /// given; it is not shown, in case it is a token given in its place.
    InvalidShop(String),

    /// Not an API version this crate supports. `version` is shown only when
    /// it is written `YYYY-MM`.
    #[non_exhaustive]
    InvalidApiVersion {
        version: String,
        reason: String,
    },

    /// The request did not reach Shopify, or its answer did not arrive.
    #[non_exhaustive]
    Transport {
        /// The address, without its user, query string and fragment.
        url: String,
        source: TransportError,
    },

    /// Shopify answered with a status other than 2xx.
    #[non_exhaustive]
    Http {
        status: u16,
        /// The address, without its user, query string and fragment.
        url: String,
        /// The `Content-Type` of the answer, as it was received. `Display`
        /// and `Debug` show it only when it is a common type.
        content_type: Option<String>,
        /// Shopify's `X-Request-Id`, which its support asks for. `Debug`
        /// shows it when it is made of hexadecimal digits and dashes.
        request_id: Option<String>,
        /// The whole body, as it was received. It may hold secrets: read it
        /// with [`Error::raw_http_body`]. Its `Debug` shows its length only.
        raw_body: RawBody,
    },

    /// The answer has a non-empty `errors` list.
    Graphql(GraphqlErrors),

    /// Shopify still throttled the request after the retries of the client's
    /// [`RetryPolicy`](crate::RetryPolicy).
    #[non_exhaustive]
    Throttled {
        retries: u32,
        cost: Option<Cost>,
        /// The wait the `Retry-After` of the last `429` asked for, if any.
        retry_after: Option<std::time::Duration>,
    },

    /// A JSON text, or the `data` of an answer, that does not fit the type
    /// it is read as: the shop runs another API version than the schema the
    /// operation was checked against, a test scripts an answer without a
    /// non-null field, a body is not JSON.
    ///
    /// `path` and `message` are written by this crate (see [`Error`]);
    /// serde's own message is in `detail`.
    #[non_exhaustive]
    Decode {
        /// The fields and variants the type declares, indexes, and `[key]`
        /// for anything else; `.` for the whole value.
        path: String,
        message: String,
        /// serde's message: it may quote the value. See
        /// [`Error::sensitive_detail`].
        detail: Option<SensitiveDetail>,
    },

    /// Variables that do not serialize, or not to an object.
    #[non_exhaustive]
    Variables {
        message: String,
        /// The message of the `Serialize` implementation that failed: it may
        /// quote the value. See [`Error::sensitive_detail`].
        detail: Option<SensitiveDetail>,
    },

    /// A token could not be obtained or renewed.
    #[non_exhaustive]
    Auth {
        message: String,
        /// What the token endpoint said, or serde's message about its
        /// answer. See [`Error::sensitive_detail`].
        detail: Option<SensitiveDetail>,
    },

    /// A mutation that this crate runs for you returned `userErrors`.
    #[non_exhaustive]
    UserErrors {
        mutation: &'static str,
        errors: Vec<UserError>,
    },

    /// A bulk operation failed, or its answer misses what was expected.
    Bulk(String),

    Timeout(String),

    /// The [`TokenStore`](crate::TokenStore) of the client failed to save a
    /// renewed token. The client holds the new token all the same.
    TokenStore(Box<dyn std::error::Error + Send + Sync>),

    /// The client has no transport: enable the `rustls` or `native-tls`
    /// feature, or give one with [`ShopifyBuilder::transport`](crate::ShopifyBuilder::transport).
    NoTransport,

    /// A webhook, an OAuth callback, an app proxy request or a session token
    /// that does not verify.
    Verification(String),

    /// A client that cannot be set up, such as a transport that does not start.
    Config(String),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidShop(_) => formatter
                .write_str("invalid shop: expected `<name>` or `<name>.myshopify.com`"),
            Self::InvalidApiVersion { version, reason } => match shown_version(version) {
                Some(version) => write!(formatter, "invalid API version `{version}`: {reason}"),
                None => write!(formatter, "invalid API version: {reason}"),
            },
            Self::Transport { url, source } => write!(formatter, "request to {url} failed: {source}"),
            Self::Http {
                status,
                url,
                content_type,
                raw_body,
                ..
            } => write!(
                formatter,
                "Shopify answered {status} to {url} ({}, {} bytes)",
                shown_content_type(content_type.as_deref()),
                raw_body.0.len()
            ),
            Self::Graphql(errors) => write!(formatter, "GraphQL errors: {errors}"),
            Self::Throttled { retries, .. } => {
                write!(formatter, "throttled by Shopify after {retries} retries")
            }
            Self::Decode { path, message, .. } => write!(
                formatter,
                "Shopify's answer does not match the operation at `{path}`: {message}"
            ),
            Self::Variables { message, .. } => write!(formatter, "invalid variables: {message}"),
            Self::Auth { message, .. } => write!(formatter, "authentication failed: {message}"),
            Self::UserErrors { mutation, errors } => {
                write!(formatter, "{mutation} refused: {}", UserErrors(errors))
            }
            Self::Bulk(message) => write!(formatter, "bulk operation: {message}"),
            Self::Timeout(message) => write!(formatter, "timed out: {message}"),
            Self::TokenStore(error) => write!(formatter, "the token store failed: {error}"),
            Self::NoTransport => formatter.write_str(
                "no HTTP transport: enable the `rustls` or `native-tls` feature, or set one with `ShopifyBuilder::transport`",
            ),
            Self::Verification(message) => write!(formatter, "verification failed: {message}"),
            Self::Config(message) => write!(formatter, "configuration: {message}"),
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidShop(shop) => formatter
                .debug_tuple("InvalidShop")
                .field(&format_args!("[{} bytes]", shop.len()))
                .finish(),
            Self::InvalidApiVersion { version, reason } => formatter
                .debug_struct("InvalidApiVersion")
                .field(
                    "version",
                    &shown_version(version).map_or_else(
                        || format!("[{} bytes]", version.len()),
                        |version| format!("{version:?}"),
                    ),
                )
                .field("reason", reason)
                .finish(),
            Self::Transport { url, source } => formatter
                .debug_struct("Transport")
                .field("url", url)
                .field("source", source)
                .finish(),
            Self::Http {
                status,
                url,
                content_type,
                request_id,
                raw_body,
            } => formatter
                .debug_struct("Http")
                .field("status", status)
                .field("url", url)
                .field(
                    "content_type",
                    &content_type
                        .as_deref()
                        .map(|content_type| shown_content_type(Some(content_type))),
                )
                .field(
                    "request_id",
                    &request_id
                        .as_deref()
                        .map(|id| if is_request_id(id) { id } else { "[not shown]" }),
                )
                .field("raw_body", raw_body)
                .finish(),
            Self::Graphql(errors) => formatter.debug_tuple("Graphql").field(errors).finish(),
            Self::Throttled {
                retries,
                cost,
                retry_after,
            } => formatter
                .debug_struct("Throttled")
                .field("retries", retries)
                .field("cost", cost)
                .field("retry_after", retry_after)
                .finish(),
            Self::Decode {
                path,
                message,
                detail,
            } => formatter
                .debug_struct("Decode")
                .field("path", path)
                .field("message", message)
                .field("detail", detail)
                .finish(),
            Self::Variables { message, detail } => formatter
                .debug_struct("Variables")
                .field("message", message)
                .field("detail", detail)
                .finish(),
            Self::Auth { message, detail } => formatter
                .debug_struct("Auth")
                .field("message", message)
                .field("detail", detail)
                .finish(),
            Self::UserErrors { mutation, errors } => formatter
                .debug_struct("UserErrors")
                .field("mutation", mutation)
                .field("errors", errors)
                .finish(),
            Self::Bulk(message) => formatter.debug_tuple("Bulk").field(message).finish(),
            Self::Timeout(message) => formatter.debug_tuple("Timeout").field(message).finish(),
            Self::TokenStore(error) => formatter.debug_tuple("TokenStore").field(error).finish(),
            Self::NoTransport => formatter.write_str("NoTransport"),
            Self::Verification(message) => formatter
                .debug_tuple("Verification")
                .field(message)
                .finish(),
            Self::Config(message) => formatter.debug_tuple("Config").field(message).finish(),
        }
    }
}

impl std::error::Error for Error {
    /// The transport's error, or the token store's: no other error is kept,
    /// so that none shows what [`Error`] does not.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport { source, .. } => Some(source),
            Self::TokenStore(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

/// `version` when it is written `YYYY-MM`: digits cannot be a token.
fn shown_version(version: &str) -> Option<&str> {
    let bytes = version.as_bytes();
    (bytes.len() == 7
        && bytes[4] == b'-'
        && bytes[..4].iter().chain(&bytes[5..]).all(u8::is_ascii_digit))
    .then_some(version)
}

/// The kind of content of an answer, from this crate's list.
fn shown_content_type(content_type: Option<&str>) -> &'static str {
    const KNOWN: [&str; 11] = [
        "application/json",
        "application/graphql-response+json",
        "application/problem+json",
        "application/jsonl",
        "application/x-ndjson",
        "application/xml",
        "application/octet-stream",
        "text/xml",
        "text/html",
        "text/plain",
        "text/csv",
    ];
    let Some(content_type) = content_type else {
        return "no content type";
    };
    let essence = content_type.split(';').next().unwrap_or_default().trim();
    KNOWN
        .iter()
        .find(|known| known.eq_ignore_ascii_case(essence))
        .copied()
        .unwrap_or("another content type")
}

/// An `X-Request-Id` as Shopify writes them: hexadecimal digits and dashes.
fn is_request_id(id: &str) -> bool {
    (1..=64).contains(&id.len())
        && id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

impl Error {
    /// The HTTP status Shopify answered with, for [`Error::Http`].
    pub fn status(&self) -> Option<u16> {
        match self {
            Self::Http { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// Shopify refused the access token: `401`.
    pub fn is_unauthorized(&self) -> bool {
        self.status() == Some(401)
    }

    /// The request ID Shopify gave, for [`Error::Http`].
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::Http { request_id, .. } => request_id.as_deref(),
            _ => None,
        }
    }

    /// The whole body of the answer, for [`Error::Http`], exactly as it was
    /// received.
    ///
    /// **It may contain secrets** (a token, a signed address): do not log it
    /// or show it to a user.
    pub fn raw_http_body(&self) -> Option<&[u8]> {
        match self {
            Self::Http { raw_body, .. } => Some(raw_body.as_bytes()),
            _ => None,
        }
    }

    /// The text this crate does not show, for [`Error::Decode`] (serde's
    /// message), [`Error::Variables`] (the message of the `Serialize`
    /// implementation that failed) and [`Error::Auth`] (what the token
    /// endpoint said).
    ///
    /// **It may contain secrets**: serde quotes the value that does not fit,
    /// a map key, or what a type's own implementation wrote, and a token
    /// endpoint may echo what it was sent. Do not log it or show it to a user.
    pub fn sensitive_detail(&self) -> Option<&str> {
        match self {
            Self::Decode { detail, .. }
            | Self::Variables { detail, .. }
            | Self::Auth { detail, .. } => detail.as_ref().map(SensitiveDetail::as_str),
            _ => None,
        }
    }

    /// The errors Shopify returned, for [`Error::Graphql`].
    pub fn graphql_errors(&self) -> &[GraphqlError] {
        match self {
            Self::Graphql(errors) => &errors.errors,
            _ => &[],
        }
    }

    pub(crate) fn transport(url: &str, source: TransportError) -> Self {
        Self::Transport {
            url: without_query(url),
            source,
        }
    }

    /// The error of an answer that is not a success.
    pub(crate) fn http(url: &str, response: &HttpResponse) -> Self {
        Self::Http {
            status: response.status,
            url: without_query(url),
            content_type: response.header("Content-Type").map(str::to_string),
            request_id: response.header("X-Request-Id").map(str::to_string),
            raw_body: RawBody(response.body.clone()),
        }
    }

    /// An error body that did not come with an [`HttpResponse`].
    pub(crate) fn http_parts(status: u16, url: &str, body: &[u8]) -> Self {
        Self::Http {
            status,
            url: without_query(url),
            content_type: None,
            request_id: None,
            raw_body: RawBody(body.to_vec()),
        }
    }

    /// An answer that does not fit its type. `message` is this crate's;
    /// `detail` is not shown.
    pub(crate) fn decode(path: impl Into<String>, message: String, detail: Option<String>) -> Self {
        Self::Decode {
            path: path.into(),
            message,
            detail: detail.map(SensitiveDetail),
        }
    }

    /// Variables that do not serialize: `variable` is the name of the one
    /// that failed, when the failure is one variable's.
    pub(crate) fn variables(variable: Option<&str>, error: &serde_json::Error) -> Self {
        let what = match variable {
            Some(name) if is_graphql_name(name) => format!("variable `${name}` does"),
            Some(_) => "a variable does".to_string(),
            None => "the variables do".to_string(),
        };
        // serde_json's own message about a map key does not quote it; any
        // other is the `Serialize` implementation's, and may quote the value.
        let why = if error.to_string().starts_with("key must be a string") {
            "a map key is not a string"
        } else {
            "a value's `Serialize` implementation failed"
        };
        Self::Variables {
            message: format!("{what} not serialize: {why}"),
            detail: Some(SensitiveDetail(error.to_string())),
        }
    }

    /// Variables that serialize to something else than an object.
    pub(crate) fn variables_not_an_object() -> Self {
        Self::Variables {
            message: "the variables must serialize to an object".to_string(),
            detail: None,
        }
    }

    pub(crate) fn auth(message: impl Into<String>) -> Self {
        Self::Auth {
            message: message.into(),
            detail: None,
        }
    }

    pub(crate) fn auth_with_detail(message: impl Into<String>, detail: String) -> Self {
        Self::Auth {
            message: message.into(),
            detail: Some(SensitiveDetail(detail)),
        }
    }
}

/// A GraphQL name: what a variable is called in an operation.
fn is_graphql_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first == b'_' || first.is_ascii_alphabetic())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

/// The body of an answer exactly as it was received, in [`Error::Http`].
///
/// **It may contain secrets.** Its `Debug` shows its length, not its content.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct RawBody(Vec<u8>);

impl RawBody {
    /// The bytes of the body. They may contain secrets: do not log them.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for RawBody {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "RawBody([{} bytes])", self.0.len())
    }
}

/// A message this crate does not show: serde's, a `Serialize`
/// implementation's, a token endpoint's. See [`Error::sensitive_detail`].
///
/// **It may contain secrets.** Its `Debug` shows its length, not its content,
/// and it has no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct SensitiveDetail(String);

impl SensitiveDetail {
    /// The message. It may contain secrets: do not log it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SensitiveDetail {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SensitiveDetail([{} bytes])", self.0.len())
    }
}

/// `url` without its user, password, query string and fragment, which may
/// hold a credential.
pub(crate) fn without_query(url: &str) -> String {
    let url = url.split(['?', '#']).next().unwrap_or_default();
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    format!("{scheme}://{host}{path}")
}

/// The `errors` of a GraphQL answer, and its `data` when it had some.
///
/// Its `Debug` shows how many entries `data` has, not their names or values,
/// which may be secret: the signed address of a bulk operation's results.
#[derive(Clone)]
#[non_exhaustive]
pub struct GraphqlErrors {
    pub errors: Vec<GraphqlError>,
    /// GraphQL may answer part of a query and fail another part.
    pub data: Option<serde_json::Value>,
}

impl fmt::Debug for GraphqlErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GraphqlErrors")
            .field("errors", &self.errors)
            .field("data", &self.data.as_ref().map(DataShape))
            .finish()
    }
}

/// How many entries partial `data` has: its keys are the answer's too.
struct DataShape<'a>(&'a serde_json::Value);

impl fmt::Debug for DataShape<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            serde_json::Value::Object(fields) => write!(formatter, "{{{} entries}}", fields.len()),
            serde_json::Value::Null => formatter.write_str("null"),
            _ => formatter.write_str(".."),
        }
    }
}

impl fmt::Display for GraphqlErrors {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.errors.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            formatter.write_str(&error.message)?;
            if let Some(code) = error.code() {
                write!(formatter, " ({code})")?;
            }
        }
        Ok(())
    }
}

struct UserErrors<'a>(&'a [UserError]);

impl fmt::Display for UserErrors<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            if let Some(field) = error.field.as_ref().filter(|field| !field.is_empty()) {
                write!(formatter, "{}: ", field.join("."))?;
            }
            formatter.write_str(&error.message)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Serialize};
    use serde_json::{Value, json};

    use super::*;
    use crate::transport::{BoxFuture, HttpRequest, Transport};

    /// A token of Shopify's shape, written in two parts so that secret
    /// scanners do not take this file for a leak.
    const TOKEN: &str = concat!("shpat", "_0123456789abcdef0123456789abcdef");
    /// `TOKEN`, each character a JSON `\u` escape.
    fn escaped_token() -> String {
        TOKEN
            .chars()
            .map(|c| format!("\\u{:04x}", c as u32))
            .collect()
    }
    /// What no error may show: the token, the parts of a signed address, a
    /// JSON-escaped token, a token in escaped quotes, a map key.
    fn secrets() -> Vec<String> {
        vec![
            TOKEN.to_string(),
            "SIGNED_SECRET".to_string(),
            "USERINFO_SECRET".to_string(),
            escaped_token(),
            "\\u0073\\u0068".to_string(),
            "QUOTED_SECRET".to_string(),
            "MAPKEY_SECRET".to_string(),
            "storage.test/k?".to_string(),
        ]
    }
    const SIGNED_URL: &str =
        "https://me:USERINFO_SECRET@storage.test/k?sig=SIGNED_SECRET#SIGNED_SECRET";

    /// A body that holds every secret, the ways round 4 found.
    fn secret_body() -> String {
        format!(
            "{{\"access_token\":\t\"{TOKEN}\",\n\"client_secret\":\n\"QUOTED_SECRET\",\
             \"url\":\"https:\\/\\/storage.test/k?a=1\\u0026sig=SIGNED_SECRET\",\
             \"echo\":\"{}\",\"q\":\"\\\"QUOTED_SECRET\\\"\",\
             \"https://storage.test/k?sig=MAPKEY_SECRET\":1}}\nX-Shopify-Access-Token:\t{TOKEN}",
            escaped_token()
        )
    }

    /// Every way `error` is shown, its sources' included.
    fn shown(error: &Error) -> String {
        let mut shown = format!("{error}\n{error:?}\n{error:#?}");
        let mut source = std::error::Error::source(error);
        while let Some(cause) = source {
            shown.push_str(&format!("\n{cause}\n{cause:?}\n{cause:#?}"));
            source = cause.source();
        }
        shown
    }

    fn assert_hides_secrets(name: &str, error: &Error) {
        let shown = shown(error);
        for secret in secrets() {
            assert!(!shown.contains(&secret), "{name} shows {secret}:\n{shown}");
        }
        // Not even decoded from JSON escapes.
        assert!(!shown.contains("shpat_"), "{name}:\n{shown}");
    }

    /// A `Serialize` and a `Deserialize` whose own message quotes a token in
    /// escaped quotes.
    struct Leaky;

    impl Serialize for Leaky {
        fn serialize<S: serde::Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(serde::ser::Error::custom(format!(
                "cannot serialize {:?}",
                format!("\"{TOKEN}\" QUOTED_SECRET")
            )))
        }
    }

    impl<'de> Deserialize<'de> for Leaky {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            let value = String::deserialize(deserializer)?;
            Err(serde::de::Error::custom(format!(
                "cannot parse value {value:?}"
            )))
        }
    }

    /// A token endpoint that answers `body` with `status`.
    struct Answer(u16, String);

    impl Transport for Answer {
        fn send(&self, _: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, TransportError>> {
            let response = HttpResponse::new(self.0, self.1.clone().into_bytes());
            Box::pin(async move { Ok(response) })
        }
    }

    fn token_endpoint(status: u16, body: String) -> Error {
        let shop = crate::ShopDomain::new("a-shop").unwrap();
        let transport = Answer(status, body);
        let future =
            crate::auth::request_token(&transport, &shop, "test", crate::auth::Encoding::Form, &[]);
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
            .unwrap_err()
    }

    /// Every variant, built the way the crate builds it, from input that
    /// holds secrets: none is shown by `Display`, `Debug`, `{:#?}` or the
    /// `source` chain.
    #[test]
    fn no_variant_shows_a_secret() {
        let body = secret_body();
        let mut errors: Vec<(&str, Error)> = Vec::new();

        errors.push(("InvalidShop", crate::ShopDomain::new(TOKEN).unwrap_err()));
        errors.push((
            "InvalidApiVersion",
            crate::ApiVersion::new(TOKEN).unwrap_err(),
        ));
        errors.push((
            "Transport",
            Error::transport(SIGNED_URL, TransportError::timeout("timed out")),
        ));
        errors.push((
            "Http",
            Error::http(
                SIGNED_URL,
                &HttpResponse::new(400, body.clone())
                    .with_header("Content-Type", format!("text/{TOKEN}"))
                    .with_header("X-Request-Id", TOKEN),
            ),
        ));
        errors.push((
            "Graphql",
            Error::Graphql(GraphqlErrors {
                errors: serde_json::from_value(json!([{
                    "message": "Variable $url is invalid",
                    "path": ["bulkOperation"],
                    "extensions": {
                        "code": "INVALID_VARIABLE",
                        "value": format!("https://storage.test/k?sig=SIGNED_SECRET {TOKEN}"),
                        "https://storage.test/k?sig=MAPKEY_SECRET": 1,
                    },
                }]))
                .unwrap(),
                data: Some(json!({
                    "https://storage.test/k?sig=MAPKEY_SECRET": TOKEN,
                    "bulkOperation": { "url": "https://storage.test/k?sig=SIGNED_SECRET" },
                })),
            }),
        ));
        errors.push((
            "Throttled",
            Error::Throttled {
                retries: 3,
                cost: None,
                retry_after: None,
            },
        ));
        // Decode: a map key, a value, a value in escaped quotes, JSON escapes,
        // invalid JSON around a token, JSONL.
        let raw = crate::RawResponse {
            data: Some(json!({ "https://storage.test/k?sig=MAPKEY_SECRET": "SIGNED_SECRET" })),
            ..Default::default()
        };
        errors.push((
            "Decode map key",
            raw.data_as::<BTreeMap<String, u64>>().unwrap_err(),
        ));
        errors.push((
            "Decode escaped quotes",
            crate::decode::from_value::<Leaky>(json!(format!("\"{TOKEN}\\\" QUOTED_SECRET")))
                .err()
                .unwrap(),
        ));
        errors.push((
            "Decode JSON escapes",
            crate::bulk::parse_jsonl::<BTreeMap<String, u64>>(
                format!(
                    "{{\"https://storage.test/k?sig=MAPKEY_SECRET\":\"{}\"}}",
                    escaped_token()
                )
                .as_bytes(),
            )
            .unwrap_err(),
        ));
        errors.push((
            "Decode invalid JSON",
            crate::decode::from_slice::<Value>(body.as_bytes())
                .err()
                .unwrap()
                .into_error(),
        ));
        errors.push((
            "Bulk count",
            crate::bulk::operation_of(&json!({
                "id": "gid://shopify/BulkOperation/1",
                "type": "QUERY",
                "status": "COMPLETED",
                "errorCode": null,
                "createdAt": "2026-01-01T00:00:00Z",
                "completedAt": null,
                "objectCount": format!("https://storage.test/k?sig=SIGNED_SECRET {TOKEN}"),
                "rootObjectCount": "0",
                "fileSize": null,
                "url": null,
                "partialDataUrl": null,
                "query": "{ shop { id } }",
            }))
            .unwrap_err(),
        ));
        let mut operation = crate::bulk::operation_of(&json!({
            "id": TOKEN,
            "type": "QUERY",
            "status": TOKEN,
            "errorCode": TOKEN,
            "createdAt": "2026-01-01T00:00:00Z",
            "completedAt": null,
            "objectCount": "1",
            "rootObjectCount": "1",
            "fileSize": null,
            "url": SIGNED_URL,
            "partialDataUrl": null,
            "query": "{ shop { id } }",
        }))
        .unwrap();
        errors.push(("Bulk ended", crate::bulk::not_completed(&operation)));
        operation.id = format!("gid://shopify/BulkOperation/{TOKEN}");
        errors.push(("Bulk ended, odd id", crate::bulk::not_completed(&operation)));
        errors.push((
            "Timeout",
            crate::bulk::timed_out(TOKEN, std::time::Duration::from_secs(1)),
        ));
        // Variables: the three ways in, one path.
        errors.push((
            "Variables of an operation",
            crate::graphql::Operation::<Value>::new("query { shop { id } }")
                .variables(BTreeMap::from([("x", Leaky)]))
                .variables_json()
                .unwrap_err(),
        ));
        errors.push((
            "Variables, one",
            crate::graphql::Operation::<Value>::new("query { shop { id } }")
                .json_variable("x", Leaky)
                .variables_json()
                .unwrap_err(),
        ));
        errors.push((
            "Variables, one with an odd name",
            crate::graphql::Operation::<Value>::new("query { shop { id } }")
                .json_variable(&format!("{TOKEN}\""), Leaky)
                .variables_json()
                .unwrap_err(),
        ));
        errors.push((
            "Variables of graphql_raw",
            crate::graphql::operation::variables_of(BTreeMap::from([("x", Leaky)])).unwrap_err(),
        ));
        // Auth: a refusal that echoes the request, a token that does not read.
        errors.push((
            "Auth refused",
            token_endpoint(
                400,
                json!({ "error": TOKEN, "error_description": body }).to_string(),
            ),
        ));
        errors.push(("Auth refused, not JSON", token_endpoint(401, body.clone())));
        errors.push((
            "Auth invalid token",
            token_endpoint(
                200,
                json!({ "access_token": 1, "scope": [TOKEN] }).to_string(),
            ),
        ));
        errors.push((
            "Auth invalid JSON",
            token_endpoint(200, format!("{{\"access_token\": \"{TOKEN}\" ")),
        ));
        errors.push((
            "UserErrors",
            Error::UserErrors {
                mutation: "bulkOperationRunQuery",
                errors: vec![UserError {
                    field: Some(vec!["query".to_string()]),
                    message: "Invalid query".to_string(),
                    code: None,
                }],
            },
        ));
        errors.push(("NoTransport", Error::NoTransport));
        errors.push(("TokenStore", Error::TokenStore("the disk is full".into())));
        errors.push((
            "Config",
            Error::Config("the HTTP client does not start".to_string()),
        ));
        #[cfg(feature = "webhooks")]
        {
            use base64::Engine;
            use hmac::{KeyInit, Mac};
            let body = b"{}";
            let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(b"secret").unwrap();
            mac.update(body);
            let signature =
                base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
            let headers = [
                ("X-Shopify-Hmac-Sha256", signature.as_str()),
                ("X-Shopify-Shop-Domain", TOKEN),
                ("X-Shopify-Topic", "orders/create"),
            ];
            errors.push((
                "Verification",
                crate::webhooks::Webhook::verify(headers, body.to_vec(), "secret").unwrap_err(),
            ));
        }

        for (name, error) in &errors {
            assert_hides_secrets(name, error);
        }
        // Each variant is there: a new one needs a case.
        let mut variants: Vec<&str> = errors.iter().map(|(_, error)| variant(error)).collect();
        variants.sort_unstable();
        variants.dedup();
        let mut expected = vec![
            "Auth",
            "Bulk",
            "Config",
            "Decode",
            "Graphql",
            "Http",
            "InvalidApiVersion",
            "InvalidShop",
            "NoTransport",
            "Throttled",
            "Timeout",
            "TokenStore",
            "Transport",
            "UserErrors",
            "Variables",
        ];
        if cfg!(feature = "webhooks") {
            expected.push("Verification");
            expected.sort_unstable();
        }
        assert_eq!(variants, expected);
    }

    /// The name of `error`'s variant: a match with no wildcard, so that a new
    /// variant does not compile until the test above covers it.
    fn variant(error: &Error) -> &'static str {
        match error {
            Error::InvalidShop(_) => "InvalidShop",
            Error::InvalidApiVersion { .. } => "InvalidApiVersion",
            Error::Transport { .. } => "Transport",
            Error::Http { .. } => "Http",
            Error::Graphql(_) => "Graphql",
            Error::Throttled { .. } => "Throttled",
            Error::Decode { .. } => "Decode",
            Error::Variables { .. } => "Variables",
            Error::Auth { .. } => "Auth",
            Error::UserErrors { .. } => "UserErrors",
            Error::Bulk(_) => "Bulk",
            Error::Timeout(_) => "Timeout",
            Error::TokenStore(_) => "TokenStore",
            Error::NoTransport => "NoTransport",
            Error::Verification(_) => "Verification",
            Error::Config(_) => "Config",
        }
    }

    #[test]
    fn what_errors_show_instead() {
        let error = Error::http(
            "https://me:pw@a.myshopify.com/admin/api/2026-10/graphql.json?x=1",
            &HttpResponse::new(500, "é".repeat(2000))
                .with_header("Content-Type", "application/json; charset=utf-8")
                .with_header("X-Request-Id", "a1b2-c3"),
        );
        assert_eq!(
            error.to_string(),
            "Shopify answered 500 to https://a.myshopify.com/admin/api/2026-10/graphql.json (application/json, 4000 bytes)"
        );
        assert!(format!("{error:?}").contains("request_id: Some(\"a1b2-c3\")"));
        assert_eq!(error.status(), Some(500));
        assert_eq!(error.request_id(), Some("a1b2-c3"));
        assert_eq!(error.raw_http_body().unwrap().len(), 4000);
        assert_eq!(error.sensitive_detail(), None);

        let raw = crate::RawResponse {
            data: Some(json!({ "orders": [{ "count": "x" }] })),
            ..Default::default()
        };
        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        struct Data {
            orders: Vec<Order>,
        }
        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        struct Order {
            count: u64,
        }
        let error = raw.data_as::<Data>().unwrap_err();
        assert_eq!(
            error.to_string(),
            "Shopify's answer does not match the operation at `orders[0].count`: invalid type: string, expected u64"
        );
        assert_eq!(
            error.sensitive_detail(),
            Some("invalid type: string \"x\", expected u64")
        );
        let debug = format!("{error:?}");
        assert!(debug.contains("detail: Some(SensitiveDetail(["), "{debug}");
        assert!(!debug.contains("\"x\""), "{debug}");

        let error = crate::ApiVersion::new("2026-02").unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("invalid API version `2026-02`: "),
            "{error}"
        );

        let error = crate::graphql::Operation::<Value>::new("query { shop { id } }")
            .json_variable("input", Leaky)
            .variables_json()
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "invalid variables: variable `$input` does not serialize: a value's `Serialize` implementation failed"
        );
        assert!(error.sensitive_detail().unwrap().contains(TOKEN));

        let error = token_endpoint(
            400,
            json!({ "error": "invalid_client", "error_description": "Client authentication failed" })
                .to_string(),
        );
        assert_eq!(
            error.to_string(),
            "authentication failed: the token endpoint of a-shop.myshopify.com answered 400 (invalid_client)"
        );
        assert_eq!(
            error.sensitive_detail(),
            Some("invalid_client: Client authentication failed")
        );
    }

    #[test]
    fn graphql_errors_keep_their_data() {
        let errors: Vec<GraphqlError> = serde_json::from_value(json!([{
            "message": "Access denied for orders field",
            "path": ["orders"],
            "extensions": { "code": "ACCESS_DENIED", "documentation": "https://x" },
        }]))
        .unwrap();
        let error = Error::Graphql(GraphqlErrors {
            errors,
            data: Some(
                json!({ "bulkOperation": { "url": "https://x.test/r?sig=S" }, "orders": null }),
            ),
        });
        let shown = format!("{error:?}");
        assert!(shown.contains("data: Some({2 entries})"), "{shown}");
        assert!(
            shown.contains("code: \"ACCESS_DENIED\", others: 1 entries"),
            "{shown}"
        );
        assert_eq!(
            error.to_string(),
            "GraphQL errors: Access denied for orders field (ACCESS_DENIED)"
        );
        let Error::Graphql(errors) = &error else {
            unreachable!()
        };
        assert!(errors.data.as_ref().unwrap()["bulkOperation"]["url"].is_string());
    }

    #[test]
    fn user_errors_name_their_field() {
        let error = Error::UserErrors {
            mutation: "webhookSubscriptionCreate",
            errors: vec![
                UserError {
                    field: Some(vec!["webhookSubscription".to_string(), "uri".to_string()]),
                    message: "Address is invalid".to_string(),
                    code: None,
                },
                UserError {
                    field: None,
                    message: "Too many".to_string(),
                    code: None,
                },
            ],
        };
        assert_eq!(
            error.to_string(),
            "webhookSubscriptionCreate refused: webhookSubscription.uri: Address is invalid; Too many"
        );
    }
}
