use crate::{utils::ReadJsonTreeSteps, Shopify, ShopifyAPIError};
use log::debug;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::bulk_query::{ShopifyBulk, ShopifyBulkOperationRunQuery, ShopifyUserError};

pub struct AutoStringJSONL {
    pub value: String,
}

impl From<serde_json::Value> for AutoStringJSONL {
    fn from(value: serde_json::Value) -> Self {
        match value {
            serde_json::Value::Array(a) => AutoStringJSONL {
                value: a
                    .into_iter()
                    .map(|v| match v {
                        serde_json::Value::Object(s) => serde_json::to_string(&s).unwrap(),
                        _ => panic!("Expected a String"),
                    })
                    .collect::<Vec<String>>()
                    .join("\n"),
            },
            _ => panic!("Expected an Array"),
        }
    }
}

impl From<String> for AutoStringJSONL {
    fn from(value: String) -> Self {
        AutoStringJSONL { value }
    }
}

impl From<&str> for AutoStringJSONL {
    fn from(value: &str) -> Self {
        AutoStringJSONL {
            value: value.to_string(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum StagedUploadTargetGenerateUploadResource {
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
    #[serde(rename = "SHOP_IMAGE")]
    ShopImage,
    #[serde(rename = "URL_REDIRECT_IMPORT")]
    UrlRedirectImport,
    #[serde(rename = "VIDEO")]
    Video,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum StagedUploadHttpMethodType {
    #[serde(rename = "POST")]
    Post,
    #[serde(rename = "PUT")]
    Put,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StagedUploadInput {
    #[serde(rename = "fileSize")]
    /// The size of the file to upload, in bytes. This is required when the request's resource property is set to VIDEO or MODEL_3D.
    pub file_size: Option<u64>,

    pub filename: String,

    #[serde(rename = "httpMethod")]
    pub http_method: Option<StagedUploadHttpMethodType>,

    pub resource: StagedUploadTargetGenerateUploadResource,

    #[serde(rename = "mimeType")]
    pub mime_type: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StagedUploadParameter {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StagedTargets {
    pub parameters: Vec<StagedUploadParameter>,

    #[serde(rename = "resourceUrl")]
    pub resource_url: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ShopifyBulkOperationRunMutation {
    #[serde(rename = "bulkOperation")]
    pub bulk_operation: Option<ShopifyBulk>,
    #[serde(rename = "userErrors")]
    pub user_errors: Option<Vec<ShopifyUserError>>,
}

impl Shopify {
    pub async fn staged_uploads_create(
        &self,
        args: Vec<StagedUploadInput>,
    ) -> Result<Vec<StagedTargets>, ShopifyAPIError> {
        self.graphql_query(
            r#"mutation stagedUploadsCreate($input: [StagedUploadInput!]!) {
            stagedUploadsCreate(input: $input) {
              stagedTargets {
                url
                resourceUrl
                parameters {
                  name
                  value
                }
              }
            }
          }
          "#,
            &json!({ "input": args }),
            &vec![
                ReadJsonTreeSteps::Key("stagedUploadsCreate"),
                ReadJsonTreeSteps::Key("stagedTargets"),
            ],
        )
        .await
    }

    pub async fn make_bulk_mutation(
        &self,
        mutation: &str,
        staged_upload_path: &str,
        client_identifier: Option<&str>,
    ) -> Result<ShopifyBulkOperationRunQuery, crate::ShopifyAPIError> {
        let bulk_query = r#"mutation bulkOperationRunMutation($mutation: String!, $stagedUploadPath: String!) {
            bulkOperationRunMutation(mutation: $mutation, stagedUploadPath: $stagedUploadPath) {
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
                bulk_query,
                &json!({
                    "mutation": mutation,
                    "stagedUploadPath": staged_upload_path,
                    "clientMutationId": client_identifier,
                }),
                &vec![
                    ReadJsonTreeSteps::Key("data"),
                    ReadJsonTreeSteps::Key("bulkOperationRunMutation"),
                ],
            )
            .await?;

        Ok(result)
    }

    pub async fn staged_uploads_submit_file_jsonl(
        parameters: &Vec<StagedUploadParameter>,
        content: AutoStringJSONL,
    ) -> Result<(), ShopifyAPIError> {
        let client = reqwest::Client::new();

        let mut form = reqwest::multipart::Form::new();
        for parameter in parameters {
            form = form.text(parameter.name.clone(), parameter.value.clone());
        }

        let file = reqwest::multipart::Part::text(content.value)
            .file_name("content.jsonl")
            .mime_str("text/jsonl")?;

        form = form.part("file", file);

        let response = client
            .post("https://shopify-staged-uploads.storage.googleapis.com/")
            .multipart(form)
            .send()
            .await?;

        let text_response = response.text().await?;

        debug!("Staged uploads submit response: {}", text_response);

        Ok(())
    }

    pub async fn prepare_execute_bulk_mutation(
        &self,
        mutation: &str,
        staged_upload_inputs: StagedUploadInput,
        client_identifier: Option<&str>,
    ) -> Result<ShopifyBulkOperationRunQuery, crate::ShopifyAPIError> {
        // Create staged uploads
        let staged_targets = self
            .staged_uploads_create(vec![staged_upload_inputs])
            .await?;
        if staged_targets.is_empty() {
            return Err(ShopifyAPIError::Other("No staged targets returned".into()));
        }

        let staged_target = staged_targets.get(0).map_or(
            Err(ShopifyAPIError::Other("No staged target returned".into())),
            Ok,
        )?;

        Shopify::staged_uploads_submit_file_jsonl(&staged_target.parameters, mutation.into())
            .await?;

        // Make and execute the bulk mutation
        let bulk_operation = self
            .make_bulk_mutation(
                mutation,
                staged_target.url.as_ref().unwrap(),
                client_identifier,
            )
            .await?;

        Ok(bulk_operation)
    }
}
