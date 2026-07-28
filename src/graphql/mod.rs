mod bulk_query;
pub mod types;

use serde::{Deserialize, Serialize};

use crate::{utils::ReadJsonTreeSteps, Shopify, ShopifyAPIError, ShopifyStorefront};

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

trait GraphqlTransport {
    fn http_client(&self) -> &reqwest::Client;
    fn query_url(&self) -> &str;
    async fn auth_header(&self) -> Result<Option<(&'static str, String)>, ShopifyAPIError>;
}

impl GraphqlTransport for Shopify {
    fn http_client(&self) -> &reqwest::Client {
        self.client()
    }

    fn query_url(&self) -> &str {
        self.get_query_url()
    }

    async fn auth_header(&self) -> Result<Option<(&'static str, String)>, ShopifyAPIError> {
        Ok(Some(("X-Shopify-Access-Token", self.access_token().await?)))
    }
}

impl GraphqlTransport for ShopifyStorefront {
    fn http_client(&self) -> &reqwest::Client {
        self.client()
    }

    fn query_url(&self) -> &str {
        self.get_query_url()
    }

    async fn auth_header(&self) -> Result<Option<(&'static str, String)>, ShopifyAPIError> {
        self.graphql_auth_header()
    }
}

async fn graphql_raw_for<Client, Variables>(
    client: &Client,
    query: &str,
    variables: &Variables,
) -> Result<GraphqlResponse<serde_json::Value>, ShopifyAPIError>
where
    Client: GraphqlTransport,
    Variables: serde::Serialize,
{
    let request = client
        .http_client()
        .post(client.query_url())
        .header("Content-Type", "application/json")
        .json(&GraphqlRequest { query, variables });
    let request = match client.auth_header().await? {
        Some((name, value)) => request.header(name, value),
        None => request,
    };
    let response = request.send().await?.error_for_status()?;

    let status = response.status();
    let body = response.text().await?;
    log::debug!("shopify graphql response status: {status}");
    serde_json::from_str(&body).map_err(ShopifyAPIError::JsonParseError)
}

async fn graphql_for<Client, ReturnType, Variables>(
    client: &Client,
    query: &str,
    variables: &Variables,
) -> Result<ReturnType, ShopifyAPIError>
where
    Client: GraphqlTransport,
    ReturnType: serde::de::DeserializeOwned,
    Variables: serde::Serialize,
{
    let response = graphql_raw_for(client, query, variables).await?;
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

async fn graphql_at_path_for<Client, ReturnType, Variables>(
    client: &Client,
    query: &str,
    variables: &Variables,
    json_finder: &[ReadJsonTreeSteps<'_>],
) -> Result<ReturnType, ShopifyAPIError>
where
    Client: GraphqlTransport,
    ReturnType: serde::de::DeserializeOwned,
    Variables: serde::Serialize,
{
    let data = graphql_for::<_, serde_json::Value, _>(client, query, variables).await?;
    let value = crate::utils::read_json_tree(&data, json_finder)
        .map_err(|_| ShopifyAPIError::NotWantedJsonFormat(data.to_string()))?;
    serde_json::from_value(value.to_owned()).map_err(ShopifyAPIError::JsonParseError)
}

#[cfg(feature = "graphql-client")]
async fn post_graphql_for<Client, Q>(
    client: &Client,
    variables: Q::Variables,
) -> Result<GraphQLClientResponse<Q::ResponseData>, ShopifyAPIError>
where
    Client: GraphqlTransport,
    Q: GraphQLQuery,
{
    let body = Q::build_query(variables);
    let request = client
        .http_client()
        .post(client.query_url())
        .header("Content-Type", "application/json")
        .json(&body);
    let request = match client.auth_header().await? {
        Some((name, value)) => request.header(name, value),
        None => request,
    };
    let response = request.send().await?.error_for_status()?;

    response
        .json::<GraphQLClientResponse<Q::ResponseData>>()
        .await
        .map_err(ShopifyAPIError::ConnectionFailed)
}

macro_rules! impl_graphql_methods {
    ($client:ty) => {
        impl $client {
            pub async fn graphql_raw<Variables>(
                &self,
                query: &str,
                variables: &Variables,
            ) -> Result<GraphqlResponse<serde_json::Value>, ShopifyAPIError>
            where
                Variables: serde::Serialize,
            {
                graphql_raw_for(self, query, variables).await
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
                graphql_for(self, query, variables).await
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
                graphql_at_path_for(self, query, variables, json_finder).await
            }

            #[cfg(feature = "graphql-client")]
            pub async fn post_graphql<Q: GraphQLQuery>(
                &self,
                variables: Q::Variables,
            ) -> Result<GraphQLClientResponse<Q::ResponseData>, ShopifyAPIError> {
                post_graphql_for::<_, Q>(self, variables).await
            }
        }
    };
}

impl_graphql_methods!(Shopify);
impl_graphql_methods!(ShopifyStorefront);

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
