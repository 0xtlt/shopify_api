use crate::{utils::ReadJsonTreeSteps, Shopify, ShopifyAPIError};
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Serialize, Deserialize)]
pub enum ShopifyBulkErrorCode {
    #[serde(rename = "ACCESS_DENIED")]
    AccessDenied,
    #[serde(rename = "INTERNAL_SERVER_ERROR")]
    InternalServerError,
    #[serde(rename = "TIMEOUT")]
    Timeout,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub enum ShopifyBulkStatus {
    #[serde(rename = "CANCELED")]
    Canceled,
    #[serde(rename = "CANCELING")]
    Canceling,
    #[serde(rename = "COMPLETED")]
    Completed,
    #[serde(rename = "CREATED")]
    Created,
    #[serde(rename = "EXPIRED")]
    Expired,
    #[serde(rename = "FAILED")]
    Failed,
    #[serde(rename = "RUNNING")]
    Running,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ShopifyBulk {
    pub id: Option<String>,
    pub url: Option<String>,
    #[serde(rename = "partialDataUrl")]
    pub partial_data_url: Option<String>,
    pub status: ShopifyBulkStatus,
    #[serde(rename = "errorCode")]
    pub error_code: Option<ShopifyBulkErrorCode>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ShopifyUserError {
    pub field: Vec<String>,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ShopifyBulkOperationRunQuery {
    #[serde(rename = "bulkOperation")]
    pub bulk_operation: Option<ShopifyBulk>,
    #[serde(rename = "userErrors")]
    pub user_errors: Option<Vec<ShopifyUserError>>,
}

impl Shopify {
    /// Query graphql shopify api
    /// # Example
    /// ```
    /// use shopify_api::*;
    /// use shopify_api::utils::ReadJsonTreeSteps;
    /// use serde::{Deserialize};
    ///
    /// #[derive(Deserialize)]
    /// struct Shop {
    ///    name: String,
    /// }
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///   let shopify = Shopify::new(env!("TEST_SHOP_NAME"), env!("TEST_KEY"), ShopifyAPIVersion::V2023_10, None);
    ///   let graphql_query = r#"{
    ///      products {
    ///         edges {
    ///          node {
    ///             id
    ///             title
    ///          }
    ///         }
    ///     }
    ///   }"#;
    ///   let variables = serde_json::json!({});
    ///   let products_bulk = shopify.make_bulk_query(graphql_query).await.unwrap();
    ///
    ///   shopify.wait_for_bulk(&products_bulk.bulk_operation.as_ref().unwrap().id.as_ref().unwrap()).await.unwrap();
    ///
    ///   let bulk_status = shopify.get_bulk_by_id(&products_bulk.bulk_operation.unwrap().id.unwrap()).await.unwrap();
    /// }
    ///
    ///
    /// ```
    pub async fn get_bulk_by_id(&self, id: &str) -> Option<ShopifyBulk> {
        let json: ShopifyBulk = self
            .graphql_query(
                r#"
                query($id: ID!) {
                    node(id: $id) {
                        ... on BulkOperation {
                            id
                            url
                            partialDataUrl
                            status
                        }
                    }
                }
            "#,
                &json!({ "id": id }),
                &vec![
                    ReadJsonTreeSteps::Key("data"),
                    ReadJsonTreeSteps::Key("node"),
                ],
            )
            .await
            .unwrap();

        Some(json)
    }
    pub async fn make_bulk_query(
        &self,
        query: &str,
    ) -> Result<ShopifyBulkOperationRunQuery, crate::ShopifyAPIError> {
        let bulk_query = format!(
            r#"
            mutation {{
                bulkOperationRunQuery(
                    query: """
                    {query}
                    """
                ) {{
                    bulkOperation {{
                        id
                        url
                        partialDataUrl
                        status
                    }}
                    userErrors {{
                        field
                        message
                    }}
                }}
            }}"#
        );

        let result: ShopifyBulkOperationRunQuery = self
            .graphql_query(
                &bulk_query,
                &json!({}),
                &vec![
                    ReadJsonTreeSteps::Key("data"),
                    ReadJsonTreeSteps::Key("bulkOperationRunQuery"),
                ],
            )
            .await?;

        Ok(result)
    }

    pub async fn wait_for_bulk(&self, id: &str) -> Result<ShopifyBulk, crate::ShopifyAPIError> {
        let mut get_bulk = self.get_bulk_by_id(id).await;

        if get_bulk.is_none() {
            return Err(crate::ShopifyAPIError::Other(
                "Bulk operation not found".to_string(),
            ));
        }

        let mut bulk = get_bulk.unwrap();

        while bulk.status == ShopifyBulkStatus::Running {
            get_bulk = self.get_bulk_by_id(id).await;

            if get_bulk.is_none() {
                return Err(crate::ShopifyAPIError::Other(
                    "Bulk operation not found".to_string(),
                ));
            }

            bulk = get_bulk.unwrap();
            tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        }

        Ok(bulk)
    }

    pub async fn download_bulk(url: &str) -> Result<Vec<serde_json::Value>, ShopifyAPIError> {
        let resp = reqwest::get(url).await?;
        let body = resp.text().await?;

        body.split('\n')
            .filter(|line| !line.is_empty())
            .map(|s| serde_json::from_str(s).map_err(ShopifyAPIError::JsonParseError))
            .collect::<Result<Vec<serde_json::Value>, _>>()
    }
}
