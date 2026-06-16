use serde_json::json;

use crate::{ApiVersion, Shopify, ShopifyAPIError};

pub const ADMIN_SCHEMA_INTROSPECTION_QUERY: &str = include_str!("../schema_dl.graphql");
pub const SHOPIFY_DEV_ADMIN_SCHEMA_PROXY: &str = "https://shopify.dev/admin-graphql-direct-proxy";

pub async fn download_public_admin_schema(
    api_version: impl AsRef<str>,
) -> Result<serde_json::Value, ShopifyAPIError> {
    let api_version = ApiVersion::new(api_version.as_ref())?;
    let url = format!("{SHOPIFY_DEV_ADMIN_SCHEMA_PROXY}/{api_version}");
    let response = reqwest::Client::new()
        .post(url)
        .json(&json!({
            "query": ADMIN_SCHEMA_INTROSPECTION_QUERY,
            "variables": {},
        }))
        .send()
        .await?
        .error_for_status()?;

    response
        .json()
        .await
        .map_err(ShopifyAPIError::ConnectionFailed)
}

impl Shopify {
    /// Downloads the Admin GraphQL schema through the authenticated shop endpoint.
    pub async fn download_admin_schema(&self) -> Result<serde_json::Value, ShopifyAPIError> {
        let response = self
            .graphql_raw(ADMIN_SCHEMA_INTROSPECTION_QUERY, &json!({}))
            .await?;

        if let Some(errors) = response.errors {
            return Err(ShopifyAPIError::GraphqlErrors(errors));
        }

        serde_json::to_value(response).map_err(ShopifyAPIError::JsonParseError)
    }
}
