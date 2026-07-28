use std::sync::{Arc, Mutex};

use thiserror::Error;

pub mod auth;
pub mod graphql;
pub mod schema;
pub mod utils;
#[cfg(feature = "webhooks")]
pub mod webhooks;

pub use auth::{ShopifyAuth, StorefrontAuth, TokenData, TokenStore};
pub use graphql::{
    BulkConcurrencyOptions, BulkOperationPayload, BulkOperationsFilter, BulkWaitOptions,
    GraphqlError, GraphqlResponse, ShopifyBulkOperation, ShopifyBulkStatus,
};
pub use schema::{download_public_admin_schema, SHOPIFY_DEV_ADMIN_SCHEMA_PROXY};

pub const DEFAULT_API_VERSION: &str = "2026-04";
pub const MIN_API_VERSION: &str = "2026-04";
pub static VERSION: &str = "shopify_api/0.10";

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct ApiVersion(String);

impl ApiVersion {
    pub fn new(version: impl Into<String>) -> Result<Self, ShopifyAPIError> {
        let version = version.into();
        validate_api_version(&version)?;
        Ok(Self(version))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for ApiVersion {
    fn default() -> Self {
        Self(DEFAULT_API_VERSION.to_string())
    }
}

impl std::fmt::Display for ApiVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TryFrom<&str> for ApiVersion {
    type Error = ShopifyAPIError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl TryFrom<String> for ApiVersion {
    type Error = ShopifyAPIError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShopifyApi {
    Admin,
    Storefront,
}

#[derive(Clone)]
pub struct ShopifyConfig {
    pub api_version: ApiVersion,
    #[cfg(feature = "webhooks")]
    pub shared_secret: Option<String>,
    pub token_store: Option<Arc<dyn TokenStore>>,
    pub token_refresh_leeway: chrono::Duration,
    pub user_agent: String,
}

impl Default for ShopifyConfig {
    fn default() -> Self {
        Self {
            api_version: ApiVersion::default(),
            #[cfg(feature = "webhooks")]
            shared_secret: None,
            token_store: None,
            token_refresh_leeway: chrono::Duration::minutes(5),
            user_agent: VERSION.to_string(),
        }
    }
}

#[derive(Clone)]
pub struct Shopify {
    pub api_version: ApiVersion,
    api: ShopifyApi,
    #[cfg(feature = "webhooks")]
    pub(crate) shared_secret: Option<String>,
    auth: Arc<Mutex<auth::ApiAuth>>,
    client: reqwest::Client,
    query_url: String,
    token_url: String,
    shop: String,
    shop_domain: String,
    token_store: Option<Arc<dyn TokenStore>>,
    token_refresh_leeway: chrono::Duration,
}

impl std::fmt::Debug for Shopify {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shopify")
            .field("api_version", &self.api_version)
            .field("api", &self.api)
            .field("query_url", &self.query_url)
            .field("token_url", &self.token_url)
            .field("shop", &self.shop)
            .field("shop_domain", &self.shop_domain)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error)]
pub enum ShopifyAPIError {
    #[error("connection failed")]
    ConnectionFailed(#[from] reqwest::Error),

    #[error("response body is broken")]
    ResponseBroken,

    #[error("not a JSON response: {0}")]
    NotJson(String),

    #[error("not wanted JSON format: {0}")]
    NotWantedJsonFormat(String),

    #[error("request throttled")]
    Throttled,

    #[error("JSON parsing error: {0}")]
    JsonParseError(#[from] serde_json::Error),

    #[error("invalid API version `{version}`: minimum supported version is {minimum}")]
    InvalidApiVersion { version: String, minimum: String },

    #[error("authentication error: {0}")]
    Authentication(String),

    #[error("GraphQL returned errors: {0:?}")]
    GraphqlErrors(Vec<GraphqlError>),

    #[error("missing GraphQL data")]
    MissingGraphqlData,

    #[error("invalid header value: {0}")]
    InvalidHeaderValue(#[from] reqwest::header::InvalidHeaderValue),

    #[error("operation timed out: {0}")]
    Timeout(String),

    #[error("{operation} is only supported by the Admin API")]
    AdminApiRequired { operation: &'static str },

    #[error("other error: {0}")]
    Other(String),
}

impl Shopify {
    pub fn new(
        shop: impl AsRef<str>,
        auth: ShopifyAuth,
        config: ShopifyConfig,
    ) -> Result<Self, ShopifyAPIError> {
        Self::new_with_auth(shop, auth::ApiAuth::Admin(auth), config)
    }

    /// Creates a client for Shopify's Storefront GraphQL API.
    ///
    /// The authentication mode determines whether requests are tokenless or use
    /// Shopify's public or private Storefront access-token header.
    pub fn new_storefront(
        shop: impl AsRef<str>,
        auth: StorefrontAuth,
        config: ShopifyConfig,
    ) -> Result<Self, ShopifyAPIError> {
        Self::new_with_auth(shop, auth::ApiAuth::Storefront(auth), config)
    }

    fn new_with_auth(
        shop: impl AsRef<str>,
        auth: auth::ApiAuth,
        config: ShopifyConfig,
    ) -> Result<Self, ShopifyAPIError> {
        let shop = shop.as_ref().to_string();
        let shop_domain = normalize_shop_domain(&shop);
        let api = match &auth {
            auth::ApiAuth::Admin(_) => ShopifyApi::Admin,
            auth::ApiAuth::Storefront(_) => ShopifyApi::Storefront,
        };
        let query_url = match api {
            ShopifyApi::Admin => format!(
                "https://{}/admin/api/{}/graphql.json",
                shop_domain, config.api_version
            ),
            ShopifyApi::Storefront => format!(
                "https://{}/api/{}/graphql.json",
                shop_domain, config.api_version
            ),
        };
        let token_url = format!("https://{}/admin/oauth/access_token", shop_domain);
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
            api,
            #[cfg(feature = "webhooks")]
            shared_secret: config.shared_secret,
            auth: Arc::new(Mutex::new(auth)),
            client,
            query_url,
            token_url,
            shop,
            shop_domain,
            token_store: config.token_store,
            token_refresh_leeway: config.token_refresh_leeway,
        })
    }

    pub fn get_shop(&self) -> &str {
        &self.shop
    }

    pub fn shop_domain(&self) -> &str {
        &self.shop_domain
    }

    pub fn api(&self) -> ShopifyApi {
        self.api
    }

    pub fn get_query_url(&self) -> &str {
        &self.query_url
    }

    pub fn token_url(&self) -> &str {
        &self.token_url
    }

    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.client
    }

    pub(crate) fn ensure_admin_api(&self, operation: &'static str) -> Result<(), ShopifyAPIError> {
        if self.api == ShopifyApi::Admin {
            Ok(())
        } else {
            Err(ShopifyAPIError::AdminApiRequired { operation })
        }
    }

    pub fn replace_auth(&self, auth: ShopifyAuth) -> Result<(), ShopifyAPIError> {
        let mut current = self
            .auth
            .lock()
            .map_err(|_| ShopifyAPIError::Authentication("auth lock poisoned".to_string()))?;
        match &mut *current {
            auth::ApiAuth::Admin(current) => {
                *current = auth;
                Ok(())
            }
            auth::ApiAuth::Storefront(_) => Err(ShopifyAPIError::Authentication(
                "cannot replace Admin authentication on a Storefront API client".to_string(),
            )),
        }
    }

    /// Replaces authentication on an existing Storefront API client.
    pub fn replace_storefront_auth(&self, auth: StorefrontAuth) -> Result<(), ShopifyAPIError> {
        let mut current = self
            .auth
            .lock()
            .map_err(|_| ShopifyAPIError::Authentication("auth lock poisoned".to_string()))?;
        match &mut *current {
            auth::ApiAuth::Storefront(current) => {
                *current = auth;
                Ok(())
            }
            auth::ApiAuth::Admin(_) => Err(ShopifyAPIError::Authentication(
                "cannot replace Storefront authentication on an Admin API client".to_string(),
            )),
        }
    }
}

fn normalize_shop_domain(shop: &str) -> String {
    let shop = shop.trim().trim_start_matches("https://");
    let shop = shop.trim_start_matches("http://").trim_end_matches('/');
    if shop.ends_with(".myshopify.com") {
        shop.to_string()
    } else {
        format!("{shop}.myshopify.com")
    }
}

fn validate_api_version(version: &str) -> Result<(), ShopifyAPIError> {
    let parsed = parse_api_version(version);
    let minimum = parse_api_version(MIN_API_VERSION);

    match (parsed, minimum) {
        (Some(parsed), Some(minimum)) if parsed >= minimum => Ok(()),
        _ => Err(ShopifyAPIError::InvalidApiVersion {
            version: version.to_string(),
            minimum: MIN_API_VERSION.to_string(),
        }),
    }
}

fn parse_api_version(version: &str) -> Option<(u16, u8)> {
    let (year, month) = version.split_once('-')?;
    Some((year.parse().ok()?, month.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn api_version_rejects_versions_before_2026_04() {
        assert!(ApiVersion::new("2025-10").is_err());
        assert!(ApiVersion::new("2026-01").is_err());
        assert_eq!(ApiVersion::new("2026-04").unwrap().as_str(), "2026-04");
        assert_eq!(ApiVersion::new("2026-07").unwrap().as_str(), "2026-07");
    }

    #[test]
    fn endpoints_are_built_from_normalized_shop_and_version() {
        let shopify = Shopify::new(
            "example",
            ShopifyAuth::AccessToken("token".to_string()),
            ShopifyConfig::default(),
        )
        .unwrap();

        assert_eq!(shopify.shop_domain(), "example.myshopify.com");
        assert_eq!(shopify.api(), ShopifyApi::Admin);
        assert_eq!(
            shopify.get_query_url(),
            "https://example.myshopify.com/admin/api/2026-04/graphql.json"
        );
        assert_eq!(
            shopify.token_url(),
            "https://example.myshopify.com/admin/oauth/access_token"
        );
    }

    #[test]
    fn storefront_endpoint_uses_the_storefront_graphql_path() {
        let shopify = Shopify::new_storefront(
            "https://example.myshopify.com/",
            StorefrontAuth::Tokenless,
            ShopifyConfig::default(),
        )
        .unwrap();

        assert_eq!(shopify.shop_domain(), "example.myshopify.com");
        assert_eq!(shopify.api(), ShopifyApi::Storefront);
        assert_eq!(
            shopify.get_query_url(),
            "https://example.myshopify.com/api/2026-04/graphql.json"
        );
    }

    #[test]
    fn authentication_can_only_be_replaced_for_the_selected_api() {
        let admin = Shopify::new(
            "example",
            ShopifyAuth::AccessToken("admin-token".to_string()),
            ShopifyConfig::default(),
        )
        .unwrap();
        let storefront = Shopify::new_storefront(
            "example",
            StorefrontAuth::Tokenless,
            ShopifyConfig::default(),
        )
        .unwrap();

        assert!(admin
            .replace_storefront_auth(StorefrontAuth::Tokenless)
            .is_err());
        assert!(storefront
            .replace_auth(ShopifyAuth::AccessToken("admin-token".to_string()))
            .is_err());
    }

    #[test]
    fn storefront_clients_reject_admin_only_operations() {
        let storefront = Shopify::new_storefront(
            "example",
            StorefrontAuth::Tokenless,
            ShopifyConfig::default(),
        )
        .unwrap();

        assert!(matches!(
            storefront.ensure_admin_api("bulk operations"),
            Err(ShopifyAPIError::AdminApiRequired {
                operation: "bulk operations"
            })
        ));
    }
}
