use crate::{utils::ReadJsonTreeSteps, Shopify, ShopifyAPIError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum StagedUploadTargetGenerateUploadResourceInput {
    #[serde(rename = "BULK_MUTATION_VARIABLES")]
    BulkMutationVariables,
    #[serde(rename = "COLLECTION_IMAGE")]
    CollectionImage,
    #[serde(rename = "FILE")]
    File,
    #[serde(rename = "IMAGE")]
    Image,
    #[serde(rename = "MODEL_3D")]
    Model3D,
    #[serde(rename = "PRODUCT_IMAGE")]
    ProductImage,
    #[serde(rename = "RETURN_LABEL")]
    ReturnLabel,
    #[serde(rename = "URL_REDIRECT_IMPORT")]
    UrlRedirectImport,
    #[serde(rename = "VIDEO")]
    Video,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StagedUploadParameter {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StagedMediaUploadTarget {
    pub parameters: Vec<StagedUploadParameter>,
    #[serde(rename = "resourceUrl")]
    pub resource_url: String,
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShopifyStagedUploadsCreateInputQuery {
    #[serde(rename = "stagedTargets")]
    pub staged_targets: Option<Vec<StagedMediaUploadTarget>>,
    #[serde(rename = "userErrors")]
    pub user_errors: Option<Vec<ShopifyUserError>>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StagedUploadsCreateInput {
    #[serde(rename = "fileSize")]
    pub file_size: Option<u64>,
    pub filename: String,

    #[serde(rename = "httpMethod")]
    pub http_method: Option<String>, // POST OR PUT

    #[serde(rename = "mimeType")]
    pub mime_type: String,

    pub resource: StagedUploadTargetGenerateUploadResourceInput,
}

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

#[derive(Debug, Serialize, Deserialize, Clone)]
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
    /// // print env!("TEST_SHOP_NAME") but replace e with 3 and a with 4
    /// println!("{}", env!("TEST_SHOP_NAME").replace("e", "3").replace("a", "4"));
    ///   let shopify = Shopify::new(env!("TEST_SHOP_NAME"), env!("TEST_KEY"), String::from("2024-04"), None);
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
    /// Executes a GraphQL bulk query on the Shopify API.
    ///
    /// # Arguments
    /// * `query` - The GraphQL query to execute in bulk.
    ///
    /// # Returns
    /// `Result<ShopifyBulkOperationRunQuery, ShopifyAPIError>` - A result containing the bulk operation or an error.
    ///
    /// # Example
    /// ```ts,no_run
    /// let bulk_query = "{ products { edges { node { id title } } } }";
    /// let result = shopify.make_bulk_query(bulk_query).await?;
    /// ```
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
    /// Executes a bulk mutation on the Shopify API.
    ///
    /// # Arguments
    /// * `mutation_string` - The string representing the mutation.
    /// * `staged_upload_path` - The path for the staged upload.
    ///
    /// # Returns
    /// `Result<ShopifyBulkOperationRunQuery, ShopifyAPIError>` - A result containing the bulk operation or an error.
    ///
    /// # Example
    /// ```ts,no_run
    /// let mutation_string = "{ updateProducts(...) { ... } }";
    /// let staged_upload_path = "/path/to/upload";
    /// let result = shopify.make_bulk_mutation(mutation_string, staged_upload_path).await?;
    /// ```
    pub async fn make_bulk_mutation(
        &self,
        mutation_string: &str,
        staged_upload_path: &str,
    ) -> Result<ShopifyBulkOperationRunQuery, crate::ShopifyAPIError> {
        let bulk_mutation = r#"
            mutation bulkOperationRunMutation($mutation: String!, $stagedUploadPath: String!) {
                bulkOperationRunMutation(
                    mutation: $mutation,
                    stagedUploadPath: $stagedUploadPath,
                ) {
                    bulkOperation {
                        id
                        url
                        partialDataUrl
                        status
                    }
                    userErrors {
                        field
                        message
                    }
                }
            }"#;

        let result: ShopifyBulkOperationRunQuery = self
            .graphql_query(
                bulk_mutation,
                &json!({
                    "mutation": mutation_string,
                    "stagedUploadPath": staged_upload_path,
                }),
                &vec![
                    ReadJsonTreeSteps::Key("data"),
                    ReadJsonTreeSteps::Key("bulkOperationRunMutation"),
                ],
            )
            .await?;

        Ok(result)
    }

    /// Waits for the specified bulk operation to complete.
    ///
    /// # Arguments
    /// * `id` - The identifier of the bulk operation.
    ///
    /// # Returns
    /// `Result<ShopifyBulk, ShopifyAPIError>` - The final status of the bulk operation or an error.
    ///
    /// # Example
    /// ```ts,no_run
    /// let bulk_id = "123456";
    /// let bulk_status = shopify.wait_for_bulk(bulk_id);
    /// ```
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

    /// Downloads data from a Shopify bulk operation.
    ///
    /// # Arguments
    /// * `url` - The URL to download the bulk operation data.
    ///
    /// # Returns
    /// `Result<Vec<serde_json::Value>, ShopifyAPIError>` - The downloaded data or an error.
    ///
    /// # Example
    /// ```ts,no_run
    /// let url = "https://shopify.com/bulk/123456";
    /// let data = Shopify::download_bulk(url).await?;
    /// ```
    pub async fn download_bulk(url: &str) -> Result<Vec<serde_json::Value>, ShopifyAPIError> {
        let resp = reqwest::get(url).await?;
        let body = resp.text().await?;

        body.split('\n')
            .filter(|line| !line.is_empty())
            .map(|s| serde_json::from_str(s).map_err(ShopifyAPIError::JsonParseError))
            .collect::<Result<Vec<serde_json::Value>, _>>()
    }

    /// Prepares a staged upload for a bulk operation.
    ///
    /// # Arguments
    /// * `params` - Parameters for the staged upload.
    ///
    /// # Returns
    /// `Result<ShopifyStagedUploadsCreateInputQuery, ShopifyAPIError>` - The result of the upload preparation or an error.
    ///
    /// # Example
    /// ```ts,no_run
    /// let params = vec![StagedUploadsCreateInput { /* ... */ }];
    /// let staged_upload = shopify.stage_upload_prepare(params).await?;
    /// ```
    pub async fn stage_upload_prepare(
        &self,
        params: Vec<StagedUploadsCreateInput>,
    ) -> Result<ShopifyStagedUploadsCreateInputQuery, ShopifyAPIError> {
        self.graphql_query(
            r#"
        mutation stagedUploadsCreate($input: [StagedUploadInput!]!) {
            stagedUploadsCreate(input: $input) {
              stagedTargets {
                url
                resourceUrl
                parameters {
                  name
                  value
                }
              }
              userErrors {
                field
                message
              }
            }
          }
          "#,
            &json!({
                "input": params
            }),
            &vec![
                ReadJsonTreeSteps::Key("data"),
                ReadJsonTreeSteps::Key("stagedUploadsCreate"),
            ],
        )
        .await
    }

    /// Generates a URL for a staged upload.
    ///
    /// # Arguments
    /// * `filename` - The name of the file to upload.
    /// * `mime_type` - The MIME type of the file.
    ///
    /// # Returns
    /// `Result<StagedMediaUploadTarget, ShopifyAPIError>` - The generated URL for the upload or an error.
    ///
    /// # Example
    /// ```ts,no_run
    /// let filename = "data.json";
    /// let mime_type = "application/json";
    /// let upload_url = shopify.generate_staged_upload_url(filename, mime_type).await?;
    /// ```
    pub async fn generate_staged_upload_url(
        &self,
        filename: &str,
        mime_type: &str,
    ) -> Result<StagedMediaUploadTarget, ShopifyAPIError> {
        let staged_upload_input = StagedUploadsCreateInput {
            file_size: None,
            filename: filename.to_string(),
            http_method: Some("POST".to_string()),
            mime_type: mime_type.to_string(),
            resource: StagedUploadTargetGenerateUploadResourceInput::BulkMutationVariables,
        };

        let response = self.stage_upload_prepare(vec![staged_upload_input]).await?;

        if let Some(errors) = response.user_errors {
            if !errors.is_empty() {
                return Err(ShopifyAPIError::Other(
                    "Error while creating staged upload URL".to_string(),
                ));
            }
        }

        if let Some(staged_targets) = response.staged_targets {
            if let Some(target) = staged_targets.first() {
                return Ok(target.clone());
            }
        }

        Err(ShopifyAPIError::Other(
            "Unable to generate staged upload URL".to_string(),
        ))
    }

    /// Uploads a JSON file for a bulk operation.
    ///
    /// # Arguments
    /// * `data` - The JSON data to upload.
    ///
    /// # Returns
    /// `Result<String, ShopifyAPIError>` - The key of the uploaded object or an error.
    ///
    /// # Example
    /// ```ts,no_run
    /// let data = vec![json!({ "key": "value" })];
    /// let key = shopify.stage_upload_json(data).await?;
    /// ```
    pub async fn stage_upload_json(&self, data: Vec<Value>) -> Result<String, ShopifyAPIError> {
        let jsonl_data = data
            .into_iter()
            .map(|item| serde_json::to_string(&item).unwrap())
            .collect::<Vec<String>>()
            .join("\n");

        let upload_url = self
            .generate_staged_upload_url("bulk_op_vars", "application/jsonl")
            .await?;

        let mut form = reqwest::multipart::Form::new();

        let mut key = String::from("");

        for param in upload_url.parameters {
            if param.name == "key" {
                key = param.value.clone();
            }

            form = form.text(param.name, param.value);
        }

        let file_part = reqwest::multipart::Part::bytes(jsonl_data.as_bytes().to_vec())
            .file_name("bulk_op_vars")
            .mime_str("application/jsonl")
            .unwrap();

        form = form.part("file", file_part);

        let client = reqwest::Client::new();
        client
            .post(&upload_url.url)
            .multipart(form)
            .send()
            .await?
            .error_for_status()?;

        Ok(key)
    }
}
