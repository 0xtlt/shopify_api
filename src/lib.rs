use thiserror::Error;

pub mod graphql;
pub mod rest;
pub mod utils;
#[cfg(feature = "webhooks")]
pub mod webhooks;

#[derive(Clone, Debug)]
pub struct Shopify {
    pub api_version: String,
    #[cfg(feature = "webhooks")]
    shared_secret: Option<String>,
    api_key: String,
    query_url: String,
    rest_url: String,
    shop: String,
}

#[derive(Debug, Error)]
pub enum ShopifyAPIError {
    #[error("Connection failed")]
    ConnectionFailed(#[from] reqwest::Error),

    #[error("Response broken")]
    ResponseBroken,

    #[error("Not a JSON response: {0}")]
    NotJson(String),

    #[error("Not wanted JSON format: {0}")]
    NotWantedJsonFormat(String),

    #[error("Throttled")]
    Throttled,

    #[error("JSON parsing error: {0}")]
    JsonParseError(#[from] serde_json::Error),

    #[error("Other error: {0}")]
    Other(String),
}

pub static VERSION: &str = "shopify_api/0.7.0";

impl Shopify {
    /// Create a new Shopify client
    /// # Example
    /// ```
    /// use shopify_api::*;
    /// let shopify = Shopify::new("myshop", "myapikey", String::from("2024-04"), Some("mysharedsecret"));
    /// // or without shared secret
    /// let shopify = Shopify::new("myshop", "myapikey", String::from("2024-04"), None);
    /// ```
    pub fn new(
        shop: &str,
        api_key: &str,
        api_version: String,
        #[cfg(feature = "webhooks")] shared_secret: Option<&str>,
    ) -> Shopify {
        let shop_domain = {
            let mut shop_domain = shop.to_string();
            if !shop_domain.ends_with(".myshopify.com") {
                shop_domain.push_str(".myshopify.com");
            }
            shop_domain
        };

        let query_url = format!(
            "https://{}/admin/api/{}/graphql.json",
            shop_domain, api_version
        );
        let rest_url = format!("https://{}/admin/api/{}/", shop_domain, api_version);

        Shopify {
            api_version,
            #[cfg(feature = "webhooks")]
            shared_secret: shared_secret.map(|secret| secret.to_string()),
            api_key: api_key.to_string(),
            query_url,
            rest_url,
            shop: shop.to_string(),
        }
    }

    /// Get the shop name
    /// # Example
    /// ```
    /// use shopify_api::*;
    /// let shopify = Shopify::new("my-shop", "my-api-key", String::from("2024-04"), Some("my-shared-secret"));
    /// assert_eq!(shopify.get_shop(), "my-shop");
    /// ```
    pub fn get_shop(&self) -> &str {
        self.shop.as_ref()
    }

    /// Set the API Key
    /// # Example
    /// ```
    /// use shopify_api::*;
    /// let mut shopify = Shopify::new("myshop", "myapikey", String::from("2024-04"), Some("mysharedsecret"));
    /// shopify.set_api_key("newapikey");
    /// ```
    /// # Errors
    /// This function returns an error if the API key is empty
    pub fn set_api_key(&mut self, api_key: &str) -> Result<&mut Shopify, String> {
        if api_key.is_empty() {
            return Err("API key cannot be empty".to_string());
        }

        self.api_key = api_key.to_string();
        Ok(self)
    }

    /// Get the query url
    pub fn get_query_url(&self) -> &str {
        self.query_url.as_ref()
    }

    /// Get the rest url
    pub fn rest_url(&self) -> &str {
        self.rest_url.as_ref()
    }

    /// Get the API endpoint
    /// # Example
    /// ```
    /// use shopify_api::*;
    /// let shopify = Shopify::new("myshop", "myapikey", String::from("2024-04"), Some("mysharedsecret"));
    ///
    /// assert_eq!(shopify.get_api_endpoint("products.json"), "https://myshop.myshopify.com/admin/api/2024-04/products.json");
    /// ```
    pub fn get_api_endpoint(&self, endpoint: &str) -> String {
        format!("{}{}", self.rest_url(), endpoint)
    }
}
