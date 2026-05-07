use serde_json::json;

use crate::{Shopify, ShopifyAPIError};

pub const ADMIN_SCHEMA_INTROSPECTION_QUERY: &str = include_str!("../schema_dl.graphql");

impl Shopify {
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
