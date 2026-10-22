//! The error of every fallible call of this crate.

use std::fmt;

use crate::graphql::{Cost, GraphqlError, UserError};
use crate::transport::{HttpResponse, TransportError};

/// `Result<T, shopify_api::Error>`.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What went wrong, with what Shopify said about it.
///
/// Secrets never appear in an error: no access token, client secret or
/// refresh token, and no query string of an address, which may be signed
/// (the results of a bulk operation).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Not a shop's handle or `myshopify.com` domain.
    #[error("invalid shop `{0}`: expected `<name>` or `<name>.myshopify.com`")]
    InvalidShop(String),

    #[error("invalid API version `{version}`: {reason}")]
    #[non_exhaustive]
    InvalidApiVersion { version: String, reason: String },

    /// The request did not reach Shopify, or its answer did not arrive.
    #[error("request to {url} failed: {source}")]
    #[non_exhaustive]
    Transport {
        /// The address, without its query string.
        url: String,
        #[source]
        source: TransportError,
    },

    /// Shopify answered with a status other than 2xx.
    #[error("Shopify answered {status} to {url}: {body}")]
    #[non_exhaustive]
    Http {
        status: u16,
        /// The address, without its query string.
        url: String,
        /// The body of the answer, cut at 2 KiB.
        body: String,
        /// Shopify's `X-Request-Id`, which its support asks for.
        request_id: Option<String>,
    },

    /// The answer has a non-empty `errors` list.
    #[error("GraphQL errors: {0}")]
    Graphql(GraphqlErrors),

    /// Shopify still throttled the request after the retries of the client's
    /// [`RetryPolicy`](crate::RetryPolicy).
    #[error("throttled by Shopify after {retries} retries")]
    #[non_exhaustive]
    Throttled {
        retries: u32,
        cost: Option<Cost>,
        /// The wait the `Retry-After` of the last `429` asked for, if any.
        retry_after: Option<std::time::Duration>,
    },

    /// The `data` of the answer does not fit the type it is read as: the
    /// shop runs another API version than the schema the operation was
    /// checked against, or a test scripts an answer without a non-null field.
    #[error("Shopify's answer does not match the operation at `{path}`: {message}")]
    #[non_exhaustive]
    Decode { path: String, message: String },

    /// A body that is not the JSON it should be.
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    /// Variables that do not serialize, or not to an object.
    #[error("invalid variables: {0}")]
    Variables(String),

    /// A token could not be obtained or renewed.
    #[error("authentication failed: {0}")]
    Auth(String),

    /// A mutation that this crate runs for you returned `userErrors`.
    #[error("{mutation} refused: {}", UserErrors(errors))]
    #[non_exhaustive]
    UserErrors {
        mutation: &'static str,
        errors: Vec<UserError>,
    },

    /// A bulk operation failed, or its answer misses what was expected.
    #[error("bulk operation: {0}")]
    Bulk(String),

    #[error("timed out: {0}")]
    Timeout(String),

    /// The [`TokenStore`](crate::TokenStore) of the client failed to save a
    /// renewed token. The client holds the new token all the same.
    #[error("the token store failed: {0}")]
    TokenStore(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The client has no transport: enable the `rustls` or `native-tls`
    /// feature, or give one with [`ShopifyBuilder::transport`](crate::ShopifyBuilder::transport).
    #[error(
        "no HTTP transport: enable the `rustls` or `native-tls` feature, or set one with `ShopifyBuilder::transport`"
    )]
    NoTransport,

    /// A webhook, an OAuth callback, an app proxy request or a session token
    /// that does not verify.
    #[error("verification failed: {0}")]
    Verification(String),

    /// A client that cannot be set up, such as a transport that does not start.
    #[error("configuration: {0}")]
    Config(String),
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
        Self::http_parts(
            response.status,
            url,
            &response.body,
            response.header("X-Request-Id").map(str::to_string),
        )
    }

    fn http_parts(status: u16, url: &str, body: &[u8], request_id: Option<String>) -> Self {
        const LIMIT: usize = 2048;
        let text = String::from_utf8_lossy(body);
        let body = if text.len() > LIMIT {
            let mut end = LIMIT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…", &text[..end])
        } else {
            text.into_owned()
        };
        Self::Http {
            status,
            url: without_query(url),
            body,
            request_id,
        }
    }
}

/// `url` without its query string and fragment, which may hold a signature.
pub(crate) fn without_query(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or_default().to_string()
}

/// The `errors` of a GraphQL answer, and its `data` when it had some.
///
/// Its `Debug` shows the names of the fields of `data`, not their values,
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

/// The top-level keys of partial `data`, without their values.
struct DataShape<'a>(&'a serde_json::Value);

impl fmt::Debug for DataShape<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            serde_json::Value::Object(fields) => {
                write!(formatter, "{{")?;
                for (index, key) in fields.keys().enumerate() {
                    if index > 0 {
                        write!(formatter, ", ")?;
                    }
                    write!(formatter, "{key:?}: ..")?;
                }
                write!(formatter, "}}")
            }
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
    use super::*;

    #[test]
    fn http_errors_cut_long_bodies_and_keep_no_signature() {
        let error = Error::http_parts(
            500,
            "https://storage.example.com/results.jsonl?X-Goog-Signature=secret#x",
            "é".repeat(2000).as_bytes(),
            Some("a1b2".to_string()),
        );
        let Error::Http { body, url, .. } = &error else {
            panic!("an HTTP error");
        };
        assert!(body.ends_with('…'));
        assert!(body.len() <= 2048 + '…'.len_utf8());
        assert_eq!(url, "https://storage.example.com/results.jsonl");
        assert!(!error.to_string().contains("secret"));
        assert_eq!(error.status(), Some(500));
        assert_eq!(error.request_id(), Some("a1b2"));
        assert!(!error.is_unauthorized());
    }

    #[test]
    fn graphql_errors_debug_hides_partial_data() {
        let errors: Vec<GraphqlError> = serde_json::from_value(serde_json::json!([{
            "message": "Access denied for orders field",
            "path": ["orders"],
            "extensions": { "code": "ACCESS_DENIED" },
        }]))
        .unwrap();
        let error = Error::Graphql(GraphqlErrors {
            errors,
            data: Some(serde_json::json!({
                "bulkOperation": {
                    "id": "gid://shopify/BulkOperation/1",
                    "url": "https://storage.googleapis.com/r.jsonl?X-Goog-Signature=SIGNED_SECRET",
                },
                "orders": null,
            })),
        });
        let shown = format!("{error:?} {error} {error:#?}");
        assert!(!shown.contains("SIGNED_SECRET"), "{shown}");
        assert!(!shown.contains("storage.googleapis.com"), "{shown}");
        assert!(shown.contains("\"bulkOperation\": .."), "{shown}");
        assert!(shown.contains("ACCESS_DENIED"), "{shown}");
        // The data itself stays available to the caller.
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
