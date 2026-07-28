use std::{
    net::IpAddr,
    sync::{Arc, Mutex},
};

use crate::{normalize_shop_domain, ApiVersion, ShopifyAPIError, VERSION};

pub(crate) const STOREFRONT_PUBLIC_ACCESS_TOKEN_HEADER: &str = "X-Shopify-Storefront-Access-Token";
pub(crate) const STOREFRONT_PRIVATE_ACCESS_TOKEN_HEADER: &str = "Shopify-Storefront-Private-Token";
pub(crate) const STOREFRONT_BUYER_IP_HEADER: &str = "Shopify-Storefront-Buyer-IP";

/// Authentication accepted by Shopify's Storefront GraphQL API.
#[derive(Clone, Eq, PartialEq)]
pub enum StorefrontAuth {
    /// Send no access token. Shopify limits the fields available to tokenless requests.
    Tokenless,
    /// Authenticate with a public Storefront access token.
    PublicAccessToken(String),
    /// Authenticate with a private Storefront access token for server-side requests.
    PrivateAccessToken(String),
}

impl std::fmt::Debug for StorefrontAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorefrontAuth::Tokenless => f.write_str("Tokenless"),
            StorefrontAuth::PublicAccessToken(_) => f
                .debug_tuple("PublicAccessToken")
                .field(&"<redacted>")
                .finish(),
            StorefrontAuth::PrivateAccessToken(_) => f
                .debug_tuple("PrivateAccessToken")
                .field(&"<redacted>")
                .finish(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct StorefrontConfig {
    pub api_version: ApiVersion,
    pub user_agent: String,
}

impl Default for StorefrontConfig {
    fn default() -> Self {
        Self {
            api_version: ApiVersion::default(),
            user_agent: VERSION.to_string(),
        }
    }
}

/// Per-request context for Storefront API calls made on behalf of a buyer.
///
/// Shopify requires the buyer IP on server-side requests that use a private
/// Storefront token and originate from buyer traffic. Static builds and other
/// requests that aren't associated with a buyer can use the regular GraphQL
/// methods without a context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorefrontRequestContext {
    buyer_ip: IpAddr,
}

impl StorefrontRequestContext {
    pub fn new(buyer_ip: IpAddr) -> Self {
        Self { buyer_ip }
    }

    pub fn buyer_ip(&self) -> IpAddr {
        self.buyer_ip
    }

    pub(crate) fn headers(&self) -> Result<reqwest::header::HeaderMap, ShopifyAPIError> {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            STOREFRONT_BUYER_IP_HEADER,
            reqwest::header::HeaderValue::from_str(&self.buyer_ip.to_string())?,
        );
        Ok(headers)
    }
}

/// A Shopify Storefront GraphQL API client.
///
/// Admin-only operations such as bulk operations, webhook management, and Admin
/// schema download are intentionally unavailable on this type.
///
/// ```compile_fail
/// use shopify_api::{ShopifyStorefront, StorefrontAuth, StorefrontConfig};
///
/// # async fn invalid() -> Result<(), shopify_api::ShopifyAPIError> {
/// let storefront = ShopifyStorefront::new(
///     "my-shop",
///     StorefrontAuth::Tokenless,
///     StorefrontConfig::default(),
/// )?;
/// storefront.run_bulk_query("{ products { edges { node { id } } } }").await?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct ShopifyStorefront {
    pub api_version: ApiVersion,
    auth: Arc<Mutex<StorefrontAuth>>,
    client: reqwest::Client,
    query_url: String,
    shop: String,
    shop_domain: String,
}

impl ShopifyStorefront {
    pub fn new(
        shop: impl AsRef<str>,
        auth: StorefrontAuth,
        config: StorefrontConfig,
    ) -> Result<Self, ShopifyAPIError> {
        let shop = shop.as_ref().to_string();
        let shop_domain = normalize_shop_domain(&shop);
        let query_url = format!(
            "https://{}/api/{}/graphql.json",
            shop_domain, config.api_version
        );
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::USER_AGENT,
            reqwest::header::HeaderValue::from_str(&config.user_agent)?,
        );
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()?;

        Ok(Self {
            api_version: config.api_version,
            auth: Arc::new(Mutex::new(auth)),
            client,
            query_url,
            shop,
            shop_domain,
        })
    }

    pub fn get_shop(&self) -> &str {
        &self.shop
    }

    pub fn shop_domain(&self) -> &str {
        &self.shop_domain
    }

    pub fn get_query_url(&self) -> &str {
        &self.query_url
    }

    pub fn replace_auth(&self, auth: StorefrontAuth) -> Result<(), ShopifyAPIError> {
        let mut current = self.auth.lock().map_err(|_| {
            ShopifyAPIError::Authentication("storefront auth lock poisoned".to_string())
        })?;
        *current = auth;
        Ok(())
    }

    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub(crate) fn graphql_auth_header(
        &self,
    ) -> Result<Option<(&'static str, String)>, ShopifyAPIError> {
        let auth = self
            .auth
            .lock()
            .map_err(|_| {
                ShopifyAPIError::Authentication("storefront auth lock poisoned".to_string())
            })?
            .clone();

        Ok(match auth {
            StorefrontAuth::Tokenless => None,
            StorefrontAuth::PublicAccessToken(token) => {
                Some((STOREFRONT_PUBLIC_ACCESS_TOKEN_HEADER, token))
            }
            StorefrontAuth::PrivateAccessToken(token) => {
                Some((STOREFRONT_PRIVATE_ACCESS_TOKEN_HEADER, token))
            }
        })
    }
}

impl std::fmt::Debug for ShopifyStorefront {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShopifyStorefront")
            .field("api_version", &self.api_version)
            .field("query_url", &self.query_url)
            .field("shop", &self.shop)
            .field("shop_domain", &self.shop_domain)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::oneshot,
    };

    async fn mock_graphql_server() -> (String, oneshot::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (request_sender, request_receiver) = oneshot::channel();

        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            let expected_length = loop {
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(count > 0, "connection closed before request was complete");
                request.extend_from_slice(&buffer[..count]);

                let Some(headers_end) = request.windows(4).position(|part| part == b"\r\n\r\n")
                else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&request[..headers_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                break headers_end + 4 + content_length;
            };

            while request.len() < expected_length {
                let count = stream.read(&mut buffer).await.unwrap();
                assert!(
                    count > 0,
                    "connection closed before request body was complete"
                );
                request.extend_from_slice(&buffer[..count]);
            }

            request_sender
                .send(String::from_utf8(request).unwrap())
                .unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 20\r\nconnection: close\r\n\r\n{\"data\":{\"shop\":{}}}",
                )
                .await
                .unwrap();
        });

        (
            format!("http://{address}/api/2026-04/graphql.json"),
            request_receiver,
        )
    }

    async fn capture_graphql_request(
        auth: StorefrontAuth,
        context: Option<&StorefrontRequestContext>,
    ) -> String {
        let (query_url, request_receiver) = mock_graphql_server().await;
        let mut storefront =
            ShopifyStorefront::new("example", auth, StorefrontConfig::default()).unwrap();
        storefront.query_url = query_url;

        match context {
            Some(context) => storefront
                .graphql_raw_with_context("query { shop { name } }", &(), context)
                .await
                .unwrap(),
            None => storefront
                .graphql_raw("query { shop { name } }", &())
                .await
                .unwrap(),
        };

        request_receiver.await.unwrap()
    }

    #[test]
    fn endpoint_uses_the_storefront_graphql_path() {
        let storefront = ShopifyStorefront::new(
            "https://example.myshopify.com/",
            StorefrontAuth::Tokenless,
            StorefrontConfig::default(),
        )
        .unwrap();

        assert_eq!(storefront.shop_domain(), "example.myshopify.com");
        assert_eq!(
            storefront.get_query_url(),
            "https://example.myshopify.com/api/2026-04/graphql.json"
        );
    }

    #[test]
    fn requests_use_the_selected_authentication() {
        let public = ShopifyStorefront::new(
            "example",
            StorefrontAuth::PublicAccessToken("public-token".to_string()),
            StorefrontConfig::default(),
        )
        .unwrap();
        let private = ShopifyStorefront::new(
            "example",
            StorefrontAuth::PrivateAccessToken("private-token".to_string()),
            StorefrontConfig::default(),
        )
        .unwrap();
        let tokenless = ShopifyStorefront::new(
            "example",
            StorefrontAuth::Tokenless,
            StorefrontConfig::default(),
        )
        .unwrap();

        assert_eq!(
            public.graphql_auth_header().unwrap(),
            Some((
                STOREFRONT_PUBLIC_ACCESS_TOKEN_HEADER,
                "public-token".to_string()
            ))
        );
        assert_eq!(
            private.graphql_auth_header().unwrap(),
            Some((
                STOREFRONT_PRIVATE_ACCESS_TOKEN_HEADER,
                "private-token".to_string()
            ))
        );
        assert_eq!(tokenless.graphql_auth_header().unwrap(), None);
    }

    #[tokio::test]
    async fn graphql_requests_send_the_selected_authentication_and_buyer_ip() {
        let public_request = capture_graphql_request(
            StorefrontAuth::PublicAccessToken("public-token".to_string()),
            None,
        )
        .await;
        let tokenless_request = capture_graphql_request(StorefrontAuth::Tokenless, None).await;
        let context = StorefrontRequestContext::new("203.0.113.10".parse::<IpAddr>().unwrap());
        let private_request = capture_graphql_request(
            StorefrontAuth::PrivateAccessToken("private-token".to_string()),
            Some(&context),
        )
        .await;

        assert!(public_request.starts_with("POST /api/2026-04/graphql.json HTTP/1.1\r\n"));
        assert!(public_request
            .to_ascii_lowercase()
            .contains("x-shopify-storefront-access-token: public-token\r\n"));
        assert!(!tokenless_request
            .to_ascii_lowercase()
            .contains("shopify-storefront"));
        assert!(private_request
            .to_ascii_lowercase()
            .contains("shopify-storefront-private-token: private-token\r\n"));
        assert!(private_request
            .to_ascii_lowercase()
            .contains("shopify-storefront-buyer-ip: 203.0.113.10\r\n"));
    }

    #[tokio::test]
    async fn authentication_replacement_changes_the_sent_header() {
        let (query_url, request_receiver) = mock_graphql_server().await;
        let mut storefront = ShopifyStorefront::new(
            "example",
            StorefrontAuth::Tokenless,
            StorefrontConfig::default(),
        )
        .unwrap();
        storefront.query_url = query_url;

        storefront
            .replace_auth(StorefrontAuth::PrivateAccessToken(
                "private-token".to_string(),
            ))
            .unwrap();

        storefront
            .graphql_raw("query { shop { name } }", &())
            .await
            .unwrap();
        let request = request_receiver.await.unwrap().to_ascii_lowercase();

        assert!(request.contains("shopify-storefront-private-token: private-token\r\n"));
    }

    #[cfg(feature = "graphql-client")]
    struct TestQuery;

    #[cfg(feature = "graphql-client")]
    impl graphql_client::GraphQLQuery for TestQuery {
        type Variables = ();
        type ResponseData = serde_json::Value;

        fn build_query(variables: Self::Variables) -> graphql_client::QueryBody<Self::Variables> {
            graphql_client::QueryBody {
                variables,
                query: "query TestQuery { shop { name } }",
                operation_name: "TestQuery",
            }
        }
    }

    #[cfg(feature = "graphql-client")]
    #[tokio::test]
    async fn typed_graphql_requests_send_the_buyer_ip() {
        let (query_url, request_receiver) = mock_graphql_server().await;
        let mut storefront = ShopifyStorefront::new(
            "example",
            StorefrontAuth::PrivateAccessToken("private-token".to_string()),
            StorefrontConfig::default(),
        )
        .unwrap();
        storefront.query_url = query_url;
        let context = StorefrontRequestContext::new("2001:db8::1".parse::<IpAddr>().unwrap());

        storefront
            .post_graphql_with_context::<TestQuery>((), &context)
            .await
            .unwrap();
        let request = request_receiver.await.unwrap().to_ascii_lowercase();

        assert!(request.contains("shopify-storefront-private-token: private-token\r\n"));
        assert!(request.contains("shopify-storefront-buyer-ip: 2001:db8::1\r\n"));
    }
}
