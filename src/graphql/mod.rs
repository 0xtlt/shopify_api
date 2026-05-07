mod bulk_query;
pub mod types;

use serde::{Deserialize, Serialize};

use crate::{utils::ReadJsonTreeSteps, Shopify, ShopifyAPIError};

pub use bulk_query::*;

#[cfg(feature = "graphql-client")]
use graphql_client::{GraphQLQuery, Response as GraphQLClientResponse};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphqlResponse<T> {
    pub data: Option<T>,
    pub errors: Option<Vec<GraphqlError>>,
    pub extensions: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphqlError {
    pub message: String,
    pub locations: Option<serde_json::Value>,
    pub path: Option<serde_json::Value>,
    pub extensions: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct GraphqlRequest<'a, Variables> {
    query: &'a str,
    variables: &'a Variables,
}

impl Shopify {
    pub async fn graphql_raw<Variables>(
        &self,
        query: &str,
        variables: &Variables,
    ) -> Result<GraphqlResponse<serde_json::Value>, ShopifyAPIError>
    where
        Variables: serde::Serialize,
    {
        let token = self.access_token().await?;
        let response = self
            .client()
            .post(self.get_query_url())
            .header("Content-Type", "application/json")
            .header("X-Shopify-Access-Token", token)
            .json(&GraphqlRequest { query, variables })
            .send()
            .await?
            .error_for_status()?;

        let body = response.text().await?;
        log::debug!("shopify graphql response: {body}");
        serde_json::from_str(&body).map_err(ShopifyAPIError::JsonParseError)
    }

    pub async fn graphql<ReturnType, Variables>(
        &self,
        query: &str,
        variables: &Variables,
    ) -> Result<ReturnType, ShopifyAPIError>
    where
        ReturnType: serde::de::DeserializeOwned,
        Variables: serde::Serialize,
    {
        let response = self.graphql_raw(query, variables).await?;
        if let Some(errors) = response.errors {
            if errors.iter().any(|error| {
                error
                    .extensions
                    .as_ref()
                    .and_then(|v| v.get("code"))
                    .and_then(|v| v.as_str())
                    == Some("THROTTLED")
            }) {
                return Err(ShopifyAPIError::Throttled);
            }
            return Err(ShopifyAPIError::GraphqlErrors(errors));
        }

        let data = response.data.ok_or(ShopifyAPIError::MissingGraphqlData)?;
        serde_json::from_value(data).map_err(ShopifyAPIError::JsonParseError)
    }

    pub async fn graphql_at_path<ReturnType, Variables>(
        &self,
        query: &str,
        variables: &Variables,
        json_finder: &[ReadJsonTreeSteps<'_>],
    ) -> Result<ReturnType, ShopifyAPIError>
    where
        ReturnType: serde::de::DeserializeOwned,
        Variables: serde::Serialize,
    {
        let data = self
            .graphql::<serde_json::Value, _>(query, variables)
            .await?;
        let value = crate::utils::read_json_tree(&data, json_finder)
            .map_err(|_| ShopifyAPIError::NotWantedJsonFormat(data.to_string()))?;
        serde_json::from_value(value.to_owned()).map_err(ShopifyAPIError::JsonParseError)
    }

    #[cfg(feature = "graphql-client")]
    pub async fn post_graphql<Q: GraphQLQuery>(
        &self,
        variables: Q::Variables,
    ) -> Result<GraphQLClientResponse<Q::ResponseData>, ShopifyAPIError> {
        let token = self.access_token().await?;
        let body = Q::build_query(variables);
        let response = self
            .client()
            .post(self.get_query_url())
            .header("Content-Type", "application/json")
            .header("X-Shopify-Access-Token", token)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        response
            .json::<GraphQLClientResponse<Q::ResponseData>>()
            .await
            .map_err(ShopifyAPIError::ConnectionFailed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphql_response_parses_errors_and_extensions() {
        let raw = r#"{
            "errors": [{"message": "boom", "extensions": {"code": "THROTTLED"}}],
            "extensions": {"cost": {"actualQueryCost": 1}}
        }"#;

        let parsed: GraphqlResponse<serde_json::Value> = serde_json::from_str(raw).unwrap();

        assert_eq!(parsed.errors.unwrap()[0].message, "boom");
        assert!(parsed.extensions.unwrap().get("cost").is_some());
    }
}
