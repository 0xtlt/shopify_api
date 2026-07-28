use std::sync::{Arc, Mutex};

use crate::{normalize_shop_domain, ApiVersion, ShopifyAPIError, VERSION};

pub(crate) const STOREFRONT_PUBLIC_ACCESS_TOKEN_HEADER: &str = "X-Shopify-Storefront-Access-Token";
pub(crate) const STOREFRONT_PRIVATE_ACCESS_TOKEN_HEADER: &str = "Shopify-Storefront-Private-Token";

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

    #[test]
    fn authentication_can_be_replaced() {
        let storefront = ShopifyStorefront::new(
            "example",
            StorefrontAuth::Tokenless,
            StorefrontConfig::default(),
        )
        .unwrap();

        storefront
            .replace_auth(StorefrontAuth::PrivateAccessToken(
                "private-token".to_string(),
            ))
            .unwrap();

        assert_eq!(
            storefront.graphql_auth_header().unwrap(),
            Some((
                STOREFRONT_PRIVATE_ACCESS_TOKEN_HEADER,
                "private-token".to_string()
            ))
        );
    }
}
