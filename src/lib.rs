use chrono::TimeZone;

pub mod graphql;
pub mod rest;
pub mod utils;

#[derive(Clone, Debug)]
pub struct Shopify {
    api_version: ShopifyAPIVersion,
    shared_secret: Option<String>,
    api_key: String,
    query_url: String,
    rest_url: String,
    shop: String,
}

#[derive(Clone, Debug)]
pub enum ShopifyAPIVersion {
    /// Deprecated
    V2021_10,
    V2022_01,
    V2022_04,
    V2022_07,
    V2022_10,

    /// Will be deprecated soon
    V2023_01,
    V2023_04,

    /// Latest stable version
    V2023_07,

    /// Release candidate
    V2023_10,
    Unstable,
}

#[derive(Clone, Debug)]
pub enum ShopifyAPIError {
    ConnectionFailed,
    ResponseBroken,
    NotJson,
    NotWantedJsonFormat,
    Throttled,
    Other(String),
}

/// Get the end of support date for a given API version
/// # Example
///
/// ```
/// use chrono::TimeZone;
/// use shopify_api::{ get_end_of_support_date, ShopifyAPIVersion };
///
/// assert_eq!(
///     get_end_of_support_date(&ShopifyAPIVersion::V2023_01),
///     chrono::Utc.ymd(2023, 1, 31).and_hms(23, 59, 59)
/// );
/// ```
pub fn get_end_of_support_date(api_version: &ShopifyAPIVersion) -> chrono::DateTime<chrono::Utc> {
    match api_version {
        ShopifyAPIVersion::V2021_10 => chrono::Utc.ymd(2021, 10, 31).and_hms(23, 59, 59),
        ShopifyAPIVersion::V2022_01 => chrono::Utc.ymd(2022, 1, 31).and_hms(23, 59, 59),
        ShopifyAPIVersion::V2022_04 => chrono::Utc.ymd(2022, 4, 30).and_hms(23, 59, 59),
        ShopifyAPIVersion::V2022_07 => chrono::Utc.ymd(2022, 7, 31).and_hms(23, 59, 59),
        ShopifyAPIVersion::V2022_10 => chrono::Utc.ymd(2022, 10, 31).and_hms(23, 59, 59),
        ShopifyAPIVersion::V2023_01 => chrono::Utc.ymd(2023, 1, 31).and_hms(23, 59, 59),
        ShopifyAPIVersion::V2023_04 => chrono::Utc.ymd(2023, 4, 30).and_hms(23, 59, 59),
        ShopifyAPIVersion::V2023_07 => chrono::Utc.ymd(2023, 7, 31).and_hms(23, 59, 59),
        ShopifyAPIVersion::V2023_10 => chrono::Utc.ymd(2023, 10, 31).and_hms(23, 59, 59),
        ShopifyAPIVersion::Unstable => chrono::Utc.ymd(9999, 12, 31).and_hms(23, 59, 59),
    }
}

/// Check if a given API version is deprecated because it is not supported anymore
/// # Example
/// ```
/// use shopify_api::{ is_deprecated, ShopifyAPIVersion };
/// assert_eq!(is_deprecated(&ShopifyAPIVersion::V2021_10), true);
/// assert_eq!(is_deprecated(&ShopifyAPIVersion::V2023_10), false);
/// ```
pub fn is_deprecated(api_version: &ShopifyAPIVersion) -> bool {
    let max_date = get_end_of_support_date(api_version);

    chrono::Utc::now() > max_date
}

/// Transform the enum type of the API version to a string
/// # Example
/// ```
/// use shopify_api::{ api_version_to_string, ShopifyAPIVersion };
/// assert_eq!(api_version_to_string(&ShopifyAPIVersion::V2021_10), "unstable"); // Deprecated
/// assert_eq!(api_version_to_string(&ShopifyAPIVersion::V2023_10), "2023-10");
/// ```
pub fn api_version_to_string(api_version: &ShopifyAPIVersion) -> String {
    if is_deprecated(api_version) {
        println!("Warning: You are using a deprecated API version");
        println!("Warning: unstable API version will be used");
        return "unstable".to_string();
    }

    match api_version {
        ShopifyAPIVersion::V2021_10 => "unstable".to_string(),
        ShopifyAPIVersion::V2022_01 => "unstable".to_string(),
        ShopifyAPIVersion::V2022_04 => "unstable".to_string(),
        ShopifyAPIVersion::V2022_07 => "unstable".to_string(),
        ShopifyAPIVersion::V2022_10 => "unstable".to_string(),
        ShopifyAPIVersion::V2023_01 => "2023-01".to_string(),
        ShopifyAPIVersion::V2023_04 => "2023-04".to_string(),
        ShopifyAPIVersion::V2023_07 => "2023-07".to_string(),
        ShopifyAPIVersion::V2023_10 => "2023-10".to_string(),
        ShopifyAPIVersion::Unstable => "unstable".to_string(),
    }
}

impl Shopify {
    /// Create a new Shopify client
    /// # Example
    /// ```
    /// use shopify_api::*;
    /// let shopify = Shopify::new("myshop", "myapikey", ShopifyAPIVersion::V2023_01, Some("mysharedsecret"));
    /// // or without shared secret
    /// let shopify = Shopify::new("myshop", "myapikey", ShopifyAPIVersion::V2023_01, None);
    /// ```
    pub fn new(
        shop: &str,
        api_key: &str,
        api_version: ShopifyAPIVersion,
        shared_secret: Option<&str>,
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
            shop_domain,
            api_version_to_string(&api_version)
        );
        let rest_url = format!(
            "https://{}/admin/api/{}/",
            shop_domain,
            api_version_to_string(&api_version)
        );

        Shopify {
            api_version,
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
    /// let shopify = Shopify::new("my-shop", "my-api-key", ShopifyAPIVersion::V2023_01, Some("my-shared-secret"));
    /// assert_eq!(shopify.get_shop(), "my-shop");
    /// ```
    pub fn get_shop(&self) -> &str {
        self.shop.as_ref()
    }

    /// Set the API Key
    /// # Example
    /// ```
    /// use shopify_api::*;
    /// let mut shopify = Shopify::new("myshop", "myapikey", ShopifyAPIVersion::V2023_01, Some("mysharedsecret"));
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
    /// let shopify = Shopify::new("myshop", "myapikey", ShopifyAPIVersion::V2023_10, Some("mysharedsecret"));
    ///
    /// assert_eq!(shopify.get_api_endpoint("products.json"), "https://myshop.myshopify.com/admin/api/2023-10/products.json");
    /// ```
    pub fn get_api_endpoint(&self, endpoint: &str) -> String {
        format!("{}{}", self.rest_url(), endpoint)
    }
}
