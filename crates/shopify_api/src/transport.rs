//! The HTTP layer, behind a trait: [`ReqwestTransport`] by default, a
//! [`MockTransport`](crate::testing::MockTransport) in tests, or your own.
//!
//! # reqwest
//!
//! No item of this crate names a type of `reqwest` but those of
//! [`reqwest_0_13`], the module named after the major version it is built
//! with. A later major version of `reqwest` comes as a module of its own,
//! next to it, in a minor release of this crate: `reqwest_0_13` stays, and
//! code that builds a client with it keeps compiling. [`ReqwestTransport::new`]
//! may move to the newer `reqwest` in a minor release.
//!
//! The default transport, and the token renewals, run on Tokio: call the
//! client from a Tokio runtime.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures_core::Stream;

/// A boxed, sendable future, as [`Transport::send`] returns.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The body of a [`StreamingResponse`], chunk by chunk. A chunk may be of
/// any size, and may end in the middle of a line or of a UTF-8 character.
pub type BodyStream = Pin<Box<dyn Stream<Item = Result<Vec<u8>, TransportError>> + Send>>;

/// Header names whose value is a secret, hidden by `Debug`.
const SECRET_HEADERS: [&str; 5] = [
    "x-shopify-access-token",
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
];

/// Headers as `Debug` shows them: secret values hidden, and the query
/// string of a `Location`, which may be a signed address, left out.
fn shown_headers(headers: &[(String, String)]) -> Vec<(&str, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let shown = if SECRET_HEADERS
                .iter()
                .any(|secret| name.eq_ignore_ascii_case(secret))
            {
                "[redacted]".to_string()
            } else if name.eq_ignore_ascii_case("location") {
                crate::error::without_query(value)
            } else {
                value.clone()
            };
            (name.as_str(), shown)
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Method {
    Get,
    Post,
}

/// A request this crate sends.
///
/// `Debug` hides the value of secret headers, the query string of the
/// address, which may sign it, and the body, which may hold a client secret
/// or a refresh token.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HttpRequest {
    pub method: Method,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// How long the whole request may take, when it differs from the transport's default.
    pub timeout: Option<Duration>,
}

impl HttpRequest {
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
            timeout: None,
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    /// A JSON body, with its `Content-Type`.
    pub(crate) fn with_json(self, body: &serde_json::Value) -> Self {
        self.with_header("Content-Type", "application/json")
            .with_body(serde_json::to_vec(body).unwrap_or_default())
    }

    /// A URL-encoded form body, with its `Content-Type`.
    pub(crate) fn with_form(self, fields: &[(&str, &str)]) -> Self {
        self.with_header("Content-Type", "application/x-www-form-urlencoded")
            .with_body(serde_urlencoded::to_string(fields).unwrap_or_default())
    }

    pub(crate) fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// The first value of a header, whatever the case of its name.
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    /// The body read as JSON, `None` when it is not.
    pub fn json(&self) -> Option<serde_json::Value> {
        serde_json::from_slice(&self.body).ok()
    }

    /// The body read as a URL-encoded form.
    pub fn form(&self) -> Vec<(String, String)> {
        serde_urlencoded::from_bytes(&self.body).unwrap_or_default()
    }
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpRequest")
            .field("method", &self.method)
            // A signed address (a bulk download) carries its credential in its query string.
            .field("url", &crate::error::without_query(&self.url))
            .field("headers", &shown_headers(&self.headers))
            .field("body", &format_args!("[{} bytes]", self.body.len()))
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// An answer to an [`HttpRequest`].
///
/// `Debug` hides the body, which may hold a token.
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("headers", &shown_headers(&self.headers))
            .field("body", &format_args!("[{} bytes]", self.body.len()))
            .finish()
    }
}

impl HttpResponse {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.into(),
        }
    }

    /// An answer with a JSON body.
    pub fn json(status: u16, body: &serde_json::Value) -> Self {
        Self::new(status, serde_json::to_vec(body).unwrap_or_default())
            .with_header("Content-Type", "application/json")
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The first value of a header, whatever the case of its name.
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    /// The body as text, what is not UTF-8 replaced.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// An answer whose body is read as it arrives, as
/// [`Transport::send_streaming`] returns it.
///
/// `Debug` hides the body, as [`HttpResponse`]'s does.
#[non_exhaustive]
pub struct StreamingResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: BodyStream,
}

impl fmt::Debug for StreamingResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StreamingResponse")
            .field("status", &self.status)
            .field("headers", &shown_headers(&self.headers))
            .finish_non_exhaustive()
    }
}

impl StreamingResponse {
    pub fn new(
        status: u16,
        body: impl Stream<Item = Result<Vec<u8>, TransportError>> + Send + 'static,
    ) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: Box::pin(body),
        }
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The first value of a header, whatever the case of its name.
    pub fn header(&self, name: &str) -> Option<&str> {
        header(&self.headers, name)
    }

    /// The answer with the first `limit` bytes of its body, the rest left
    /// unread: what an error keeps of a body that may be of any size.
    pub(crate) async fn into_response(mut self, limit: usize) -> HttpResponse {
        let mut body = Vec::new();
        while body.len() < limit {
            match next_chunk(&mut self.body).await {
                Some(Ok(chunk)) => {
                    let kept = chunk.len().min(limit - body.len());
                    body.extend_from_slice(&chunk[..kept]);
                }
                Some(Err(_)) | None => break,
            }
        }
        HttpResponse {
            status: self.status,
            headers: self.headers,
            body,
        }
    }
}

impl From<HttpResponse> for StreamingResponse {
    fn from(response: HttpResponse) -> Self {
        let body = (!response.body.is_empty()).then_some(Ok(response.body));
        Self {
            status: response.status,
            headers: response.headers,
            body: chunks(body),
        }
    }
}

pub(crate) async fn next_chunk(body: &mut BodyStream) -> Option<Result<Vec<u8>, TransportError>> {
    std::future::poll_fn(|context| body.as_mut().poll_next(context)).await
}

/// A body made of the chunks `iter` gives, produced as they are polled.
pub(crate) fn chunks<I>(iter: I) -> BodyStream
where
    I: IntoIterator<Item = Result<Vec<u8>, TransportError>>,
    I::IntoIter: Send + 'static,
{
    Box::pin(Chunks(iter.into_iter()))
}

struct Chunks<I>(I);

// The iterator is never pinned: it is only called through `&mut`.
impl<I> Unpin for Chunks<I> {}

impl<I: Iterator<Item = Result<Vec<u8>, TransportError>>> Stream for Chunks<I> {
    type Item = Result<Vec<u8>, TransportError>;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Ready(self.0.next())
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(given, _)| given.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

/// A request that did not get an answer: no connection, a timeout, a body
/// that stopped halfway.
#[derive(Debug)]
pub struct TransportError {
    message: String,
    timeout: bool,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl TransportError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            timeout: false,
            source: None,
        }
    }

    /// The request took longer than it may.
    pub fn timeout(message: impl Into<String>) -> Self {
        Self {
            timeout: true,
            ..Self::new(message)
        }
    }

    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    pub fn is_timeout(&self) -> bool {
        self.timeout
    }
}

impl fmt::Display for TransportError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for TransportError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_deref().map(|source| source as _)
    }
}

/// Sends the requests of a [`Shopify`](crate::Shopify) client.
///
/// Requests carry secrets in their headers or their body: an implementation
/// must not follow redirections, and returns a `3xx` as the answer, so that a
/// secret is never sent on to another host.
pub trait Transport: Send + Sync + 'static {
    fn send(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, TransportError>>;

    /// Sends `request` and returns the answer as soon as its headers
    /// arrive, its body read as the caller polls it. The bulk downloads go
    /// through it, so that a file of gigabytes is never held in memory.
    ///
    /// When it streams, `request.timeout` bounds the wait for the headers
    /// and for each chunk, not the whole body, which is read at its
    /// consumer's pace.
    ///
    /// The default sends `request` with [`Transport::send`] and returns the
    /// whole body as one chunk: a transport that implements only `send`
    /// works, with the body in memory and `request.timeout` for the whole
    /// exchange.
    fn send_streaming(
        &self,
        request: HttpRequest,
    ) -> BoxFuture<'_, Result<StreamingResponse, TransportError>> {
        Box::pin(async move { self.send(request).await.map(StreamingResponse::from) })
    }
}

/// A [`ReqwestTransport`] from a client of `reqwest` 0.13 you configure.
///
/// ```
/// use shopify_api::transport::reqwest_0_13::{self, reqwest};
///
/// let builder = reqwest::Client::builder().connect_timeout(std::time::Duration::from_secs(3));
/// let transport = reqwest_0_13::from_builder(builder)?;
/// # Ok::<(), shopify_api::Error>(())
/// ```
#[cfg(any(feature = "rustls", feature = "native-tls"))]
#[cfg_attr(docsrs, doc(cfg(any(feature = "rustls", feature = "native-tls"))))]
pub mod reqwest_0_13 {
    use super::ReqwestTransport;

    /// The `reqwest` 0.13 this crate is built with: build the client of
    /// [`from_builder`] with it.
    pub use reqwest;

    /// A transport from a client you configure: a proxy, other timeouts,
    /// other certificates. Redirections are turned off whatever the builder
    /// says, as [`Transport`](super::Transport) requires.
    pub fn from_builder(builder: reqwest::ClientBuilder) -> Result<ReqwestTransport, crate::Error> {
        let client = builder
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                // A proxy's address may hold a password: errors leave it out.
                crate::Error::Config(format!(
                    "the HTTP client does not start: {}",
                    error.without_url()
                ))
            })?;
        Ok(ReqwestTransport { client })
    }
}

/// The default [`Transport`], over `reqwest`: no redirections, a 10-second
/// connection timeout and a 60-second request timeout. A streamed answer
/// with a timeout of its own waits that long for its headers and for each
/// chunk instead, however long the whole body takes.
#[cfg(any(feature = "rustls", feature = "native-tls"))]
#[cfg_attr(docsrs, doc(cfg(any(feature = "rustls", feature = "native-tls"))))]
#[derive(Debug, Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

#[cfg(any(feature = "rustls", feature = "native-tls"))]
impl ReqwestTransport {
    /// To configure the client, see [`reqwest_0_13::from_builder`].
    pub fn new() -> Result<Self, crate::Error> {
        reqwest_0_13::from_builder(
            reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(60)),
        )
    }
}

#[cfg(any(feature = "rustls", feature = "native-tls"))]
impl ReqwestTransport {
    fn builder(&self, request: HttpRequest) -> reqwest::RequestBuilder {
        let method = match request.method {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
        };
        let mut builder = self.client.request(method, &request.url);
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(timeout) = request.timeout {
            builder = builder.timeout(timeout);
        }
        if !request.body.is_empty() {
            builder = builder.body(request.body);
        }
        builder
    }
}

#[cfg(any(feature = "rustls", feature = "native-tls"))]
fn reqwest_failed(error: reqwest::Error) -> TransportError {
    // Its address may hold a signature: errors leave it out.
    let error = error.without_url();
    let message = error.to_string();
    if error.is_timeout() {
        TransportError::timeout(message).with_source(error)
    } else {
        TransportError::new(message).with_source(error)
    }
}

#[cfg(any(feature = "rustls", feature = "native-tls"))]
fn reqwest_headers(response: &reqwest::Response) -> Vec<(String, String)> {
    response
        .headers()
        .iter()
        .filter_map(|(name, value)| Some((name.to_string(), value.to_str().ok()?.to_string())))
        .collect()
}

#[cfg(any(feature = "rustls", feature = "native-tls"))]
impl Transport for ReqwestTransport {
    fn send(&self, request: HttpRequest) -> BoxFuture<'_, Result<HttpResponse, TransportError>> {
        Box::pin(async move {
            let response = self.builder(request).send().await.map_err(reqwest_failed)?;
            let status = response.status().as_u16();
            let headers = reqwest_headers(&response);
            let body = Vec::from(response.bytes().await.map_err(reqwest_failed)?);
            Ok(HttpResponse {
                status,
                headers,
                body,
            })
        })
    }

    fn send_streaming(
        &self,
        request: HttpRequest,
    ) -> BoxFuture<'_, Result<StreamingResponse, TransportError>> {
        Box::pin(async move {
            let wait = request.timeout;
            let mut builder = self.builder(request);
            let response = match wait {
                Some(wait) => {
                    // reqwest's timeout would cover the whole body: it is
                    // lifted, and `wait` bounds the headers and each chunk.
                    builder = builder.timeout(Duration::MAX);
                    tokio::time::timeout(wait, builder.send())
                        .await
                        .map_err(|_| {
                            TransportError::timeout(format!("no answer within {wait:?}"))
                        })?
                }
                None => builder.send().await,
            }
            .map_err(reqwest_failed)?;
            Ok(StreamingResponse {
                status: response.status().as_u16(),
                headers: reqwest_headers(&response),
                body: Box::pin(ReqwestBody {
                    response: Some(response),
                    pending: None,
                    wait,
                }),
            })
        })
    }
}

#[cfg(any(feature = "rustls", feature = "native-tls"))]
type PendingChunk =
    BoxFuture<'static, (reqwest::Response, Result<Option<Vec<u8>>, TransportError>)>;

/// The body of a reqwest answer as a [`Stream`], read with
/// `Response::chunk`, which needs no feature of reqwest.
#[cfg(any(feature = "rustls", feature = "native-tls"))]
struct ReqwestBody {
    response: Option<reqwest::Response>,
    pending: Option<PendingChunk>,
    wait: Option<Duration>,
}

#[cfg(any(feature = "rustls", feature = "native-tls"))]
impl Stream for ReqwestBody {
    type Item = Result<Vec<u8>, TransportError>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            if let Some(pending) = self.pending.as_mut() {
                let (response, chunk) = match pending.as_mut().poll(context) {
                    Poll::Ready(done) => done,
                    Poll::Pending => return Poll::Pending,
                };
                self.pending = None;
                return Poll::Ready(match chunk {
                    Ok(Some(chunk)) => {
                        self.response = Some(response);
                        Some(Ok(chunk))
                    }
                    Ok(None) => None,
                    Err(error) => Some(Err(error)),
                });
            }
            let Some(mut response) = self.response.take() else {
                return Poll::Ready(None);
            };
            let wait = self.wait;
            // The timer starts when the chunk is asked for: a slow consumer
            // does not count against it.
            self.pending = Some(Box::pin(async move {
                let chunk = match wait {
                    Some(wait) => match tokio::time::timeout(wait, response.chunk()).await {
                        Ok(chunk) => chunk.map_err(reqwest_failed),
                        Err(_) => Err(TransportError::timeout(format!(
                            "the body stalled for {wait:?}"
                        ))),
                    },
                    None => response.chunk().await.map_err(reqwest_failed),
                };
                (response, chunk.map(|chunk| chunk.map(Vec::from)))
            }));
        }
    }
}

/// The transport a client uses when it is given none.
pub(crate) fn default_transport() -> Result<std::sync::Arc<dyn Transport>, crate::Error> {
    #[cfg(any(feature = "rustls", feature = "native-tls"))]
    {
        Ok(std::sync::Arc::new(ReqwestTransport::new()?))
    }
    #[cfg(not(any(feature = "rustls", feature = "native-tls")))]
    {
        Err(crate::Error::NoTransport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_secrets() {
        let request = HttpRequest::new(
            Method::Post,
            "https://shop.myshopify.com/admin/oauth/access_token",
        )
        .with_header("X-Shopify-Access-Token", "shpat_secret")
        .with_form(&[("client_secret", "very-secret")]);
        let shown = format!("{request:?}");
        assert!(!shown.contains("shpat_secret"), "{shown}");
        assert!(!shown.contains("very-secret"), "{shown}");
        assert!(shown.contains("[redacted]"), "{shown}");
        assert_eq!(
            request.header("x-shopify-access-token"),
            Some("shpat_secret")
        );
        assert_eq!(
            request.form(),
            [("client_secret".to_string(), "very-secret".to_string())]
        );
    }

    /// A signed download address carries its credential in its query string.
    #[test]
    fn debug_hides_the_query_string_of_the_url() {
        let request = HttpRequest::new(
            Method::Get,
            "https://storage.googleapis.com/export.jsonl?X-Goog-Signature=SECRET#frag",
        );
        let shown = format!("{request:?}");
        assert!(!shown.contains("SECRET"), "{shown}");
        assert!(
            shown.contains("https://storage.googleapis.com/export.jsonl"),
            "{shown}"
        );
        assert_eq!(
            request.url,
            "https://storage.googleapis.com/export.jsonl?X-Goog-Signature=SECRET#frag"
        );

        let response = HttpResponse::new(302, "")
            .with_header("Set-Cookie", "session=SECRET")
            .with_header("Location", "https://storage.googleapis.com/x?sig=SECRET");
        let shown = format!("{response:?}");
        assert!(!shown.contains("SECRET"), "{shown}");
    }

    #[test]
    fn responses_read_headers_whatever_their_case() {
        let response = HttpResponse::json(429, &serde_json::json!({ "errors": "Throttled" }))
            .with_header("Retry-After", "2.0");
        assert_eq!(response.header("retry-after"), Some("2.0"));
        assert_eq!(response.header("content-type"), Some("application/json"));
        assert!(!response.is_success());
        assert_eq!(response.text(), "{\"errors\":\"Throttled\"}");
    }

    #[tokio::test]
    async fn an_error_body_copies_only_what_it_keeps() {
        let huge = vec![b'x'; 32 * 1024 * 1024];
        let response = StreamingResponse::new(500, chunks([Ok(huge)]))
            .into_response(1024)
            .await;
        assert_eq!(response.body.len(), 1024);
        assert!(
            response.body.capacity() <= 1024,
            "{} bytes allocated",
            response.body.capacity()
        );

        let parts = [
            Ok(b"abc".to_vec()),
            Ok(b"defgh".to_vec()),
            Ok(b"ij".to_vec()),
        ];
        let response = StreamingResponse::new(500, chunks(parts))
            .into_response(6)
            .await;
        assert_eq!(response.body, b"abcdef");
    }
}
