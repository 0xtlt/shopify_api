use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{Shopify, ShopifyAPIError};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StagedUploadResource {
    BulkMutationVariables,
    CollectionImage,
    File,
    Image,
    Model3D,
    ProductImage,
    ReturnLabel,
    UrlRedirectImport,
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
#[serde(rename_all = "camelCase")]
pub struct StagedUploadsCreateInput {
    pub filename: String,
    pub mime_type: String,
    pub resource: StagedUploadResource,
    pub http_method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_size: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShopifyUserError {
    pub field: Option<Vec<String>>,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ShopifyBulkErrorCode {
    AccessDenied,
    InternalServerError,
    Timeout,
}

#[derive(Debug, Serialize, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ShopifyBulkStatus {
    Canceled,
    Canceling,
    Completed,
    Created,
    Expired,
    Failed,
    Running,
}

impl ShopifyBulkStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ShopifyBulkStatus::Canceled
                | ShopifyBulkStatus::Completed
                | ShopifyBulkStatus::Expired
                | ShopifyBulkStatus::Failed
        )
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShopifyBulkOperation {
    pub id: String,
    pub url: Option<String>,
    #[serde(rename = "partialDataUrl")]
    pub partial_data_url: Option<String>,
    pub status: ShopifyBulkStatus,
    #[serde(rename = "errorCode")]
    pub error_code: Option<ShopifyBulkErrorCode>,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(rename = "completedAt")]
    pub completed_at: Option<String>,
    #[serde(rename = "objectCount")]
    pub object_count: Option<String>,
    #[serde(rename = "fileSize")]
    pub file_size: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BulkOperationPayload {
    #[serde(rename = "bulkOperation")]
    pub bulk_operation: Option<ShopifyBulkOperation>,
    #[serde(rename = "userErrors")]
    pub user_errors: Vec<ShopifyUserError>,
}

#[derive(Debug, Clone)]
pub struct BulkWaitOptions {
    pub poll_interval: Duration,
    pub timeout: Option<Duration>,
}

impl Default for BulkWaitOptions {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(10),
            timeout: None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct BulkOperationsFilter {
    pub first: u32,
    pub query: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Clone)]
pub struct BulkConcurrencyOptions {
    pub max_concurrent: usize,
}

impl Default for BulkConcurrencyOptions {
    fn default() -> Self {
        Self { max_concurrent: 5 }
    }
}

#[derive(Debug, Deserialize)]
struct RunBulkQueryData {
    #[serde(rename = "bulkOperationRunQuery")]
    payload: BulkOperationPayload,
}

#[derive(Debug, Deserialize)]
struct RunBulkMutationData {
    #[serde(rename = "bulkOperationRunMutation")]
    payload: BulkOperationPayload,
}

#[derive(Debug, Deserialize)]
struct BulkOperationData {
    #[serde(rename = "bulkOperation")]
    bulk_operation: Option<ShopifyBulkOperation>,
}

#[derive(Debug, Deserialize)]
struct BulkOperationsData {
    #[serde(rename = "bulkOperations")]
    bulk_operations: BulkOperationConnection,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BulkOperationConnection {
    pub edges: Vec<BulkOperationEdge>,
    #[serde(rename = "pageInfo")]
    pub page_info: PageInfo,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BulkOperationEdge {
    pub cursor: String,
    pub node: ShopifyBulkOperation,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct PageInfo {
    #[serde(rename = "hasNextPage")]
    pub has_next_page: bool,
    #[serde(rename = "endCursor")]
    pub end_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StagedUploadsData {
    #[serde(rename = "stagedUploadsCreate")]
    staged_uploads_create: StagedUploadsCreatePayload,
}

#[derive(Debug, Deserialize)]
struct StagedUploadsCreatePayload {
    #[serde(rename = "stagedTargets")]
    staged_targets: Vec<StagedMediaUploadTarget>,
    #[serde(rename = "userErrors")]
    user_errors: Vec<ShopifyUserError>,
}

impl Shopify {
    pub async fn run_bulk_queries<I, S>(
        &self,
        queries: I,
        options: BulkConcurrencyOptions,
    ) -> Result<Vec<BulkOperationPayload>, ShopifyAPIError>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let queries = queries.into_iter().collect::<Vec<_>>();
        if queries.len() > options.max_concurrent {
            return Err(ShopifyAPIError::Other(format!(
                "received {} bulk queries but max_concurrent is {}",
                queries.len(),
                options.max_concurrent
            )));
        }

        let mut operations = Vec::with_capacity(queries.len());
        for query in queries {
            operations.push(self.run_bulk_query(query.as_ref()).await?);
        }

        Ok(operations)
    }

    pub async fn run_bulk_mutations<I, M, P>(
        &self,
        mutations: I,
        options: BulkConcurrencyOptions,
    ) -> Result<Vec<BulkOperationPayload>, ShopifyAPIError>
    where
        I: IntoIterator<Item = (M, P)>,
        M: AsRef<str>,
        P: AsRef<str>,
    {
        let mutations = mutations.into_iter().collect::<Vec<_>>();
        if mutations.len() > options.max_concurrent {
            return Err(ShopifyAPIError::Other(format!(
                "received {} bulk mutations but max_concurrent is {}",
                mutations.len(),
                options.max_concurrent
            )));
        }

        let mut operations = Vec::with_capacity(mutations.len());
        for (mutation, staged_upload_path) in mutations {
            operations.push(
                self.run_bulk_mutation(mutation.as_ref(), staged_upload_path.as_ref())
                    .await?,
            );
        }

        Ok(operations)
    }

    pub async fn run_bulk_query(
        &self,
        query: &str,
    ) -> Result<BulkOperationPayload, ShopifyAPIError> {
        self.run_bulk_query_with_grouping(query, false).await
    }

    pub async fn run_bulk_query_with_grouping(
        &self,
        query: &str,
        group_objects: bool,
    ) -> Result<BulkOperationPayload, ShopifyAPIError> {
        let data: RunBulkQueryData = self
            .graphql(
                r#"
                mutation bulkOperationRunQuery($query: String!, $groupObjects: Boolean!) {
                    bulkOperationRunQuery(query: $query, groupObjects: $groupObjects) {
                        bulkOperation {
                            id
                            status
                            url
                            partialDataUrl
                        }
                        userErrors {
                            field
                            message
                        }
                    }
                }
                "#,
                &json!({
                    "query": query,
                    "groupObjects": group_objects,
                }),
            )
            .await?;

        Ok(data.payload)
    }

    pub async fn run_bulk_mutation(
        &self,
        mutation: &str,
        staged_upload_path: &str,
    ) -> Result<BulkOperationPayload, ShopifyAPIError> {
        let data: RunBulkMutationData = self
            .graphql(
                r#"
                mutation bulkOperationRunMutation($mutation: String!, $stagedUploadPath: String!) {
                    bulkOperationRunMutation(
                        mutation: $mutation,
                        stagedUploadPath: $stagedUploadPath
                    ) {
                        bulkOperation {
                            id
                            status
                            url
                            partialDataUrl
                        }
                        userErrors {
                            field
                            message
                        }
                    }
                }
                "#,
                &json!({
                    "mutation": mutation,
                    "stagedUploadPath": staged_upload_path,
                }),
            )
            .await?;

        Ok(data.payload)
    }

    pub async fn get_bulk_operation(
        &self,
        id: &str,
    ) -> Result<Option<ShopifyBulkOperation>, ShopifyAPIError> {
        let data: BulkOperationData = self
            .graphql(
                r#"
                query bulkOperation($id: ID!) {
                    bulkOperation(id: $id) {
                        id
                        status
                        errorCode
                        createdAt
                        completedAt
                        objectCount
                        fileSize
                        url
                        partialDataUrl
                    }
                }
                "#,
                &json!({ "id": id }),
            )
            .await?;

        Ok(data.bulk_operation)
    }

    pub async fn list_bulk_operations(
        &self,
        filter: BulkOperationsFilter,
    ) -> Result<BulkOperationConnection, ShopifyAPIError> {
        let first = if filter.first == 0 { 10 } else { filter.first };
        let data: BulkOperationsData = self
            .graphql(
                r#"
                query bulkOperations($first: Int!, $query: String, $after: String) {
                    bulkOperations(first: $first, query: $query, after: $after) {
                        edges {
                            cursor
                            node {
                                id
                                status
                                errorCode
                                createdAt
                                completedAt
                                objectCount
                                fileSize
                                url
                                partialDataUrl
                            }
                        }
                        pageInfo {
                            hasNextPage
                            endCursor
                        }
                    }
                }
                "#,
                &json!({
                    "first": first,
                    "query": filter.query,
                    "after": filter.after,
                }),
            )
            .await?;

        Ok(data.bulk_operations)
    }

    pub async fn wait_for_bulk(
        &self,
        id: &str,
        options: BulkWaitOptions,
    ) -> Result<ShopifyBulkOperation, ShopifyAPIError> {
        let started_at = Instant::now();

        loop {
            let operation = self.get_bulk_operation(id).await?.ok_or_else(|| {
                ShopifyAPIError::Other(format!("bulk operation `{id}` was not found"))
            })?;

            if operation.status.is_terminal() {
                return Ok(operation);
            }

            if let Some(timeout) = options.timeout {
                if started_at.elapsed() >= timeout {
                    return Err(ShopifyAPIError::Timeout(format!(
                        "bulk operation `{id}` did not finish within {timeout:?}"
                    )));
                }
            }

            tokio::time::sleep(options.poll_interval).await;
        }
    }

    pub async fn stage_upload_jsonl<T>(&self, data: &[T]) -> Result<String, ShopifyAPIError>
    where
        T: Serialize,
    {
        let jsonl_data = data
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<String>, _>>()?
            .join("\n");

        let target = self
            .create_staged_upload("bulk_op_vars", "text/jsonl")
            .await?;

        let mut staged_upload_path = String::new();
        let mut form = reqwest::multipart::Form::new();

        for parameter in target.parameters {
            if parameter.name == "key" {
                staged_upload_path = parameter.value.clone();
            }
            form = form.text(parameter.name, parameter.value);
        }

        let file_part = reqwest::multipart::Part::bytes(jsonl_data.into_bytes())
            .file_name("bulk_op_vars")
            .mime_str("text/jsonl")
            .map_err(|err| ShopifyAPIError::Other(err.to_string()))?;

        form = form.part("file", file_part);
        self.client()
            .post(&target.url)
            .multipart(form)
            .send()
            .await?
            .error_for_status()?;

        if staged_upload_path.is_empty() {
            return Err(ShopifyAPIError::Other(
                "staged upload did not return a key parameter".to_string(),
            ));
        }

        Ok(staged_upload_path)
    }

    pub async fn download_bulk_jsonl<T>(url: &str) -> Result<Vec<T>, ShopifyAPIError>
    where
        T: serde::de::DeserializeOwned,
    {
        let body = reqwest::get(url).await?.error_for_status()?.text().await?;
        parse_jsonl(&body)
    }

    async fn create_staged_upload(
        &self,
        filename: &str,
        mime_type: &str,
    ) -> Result<StagedMediaUploadTarget, ShopifyAPIError> {
        let input = StagedUploadsCreateInput {
            filename: filename.to_string(),
            mime_type: mime_type.to_string(),
            resource: StagedUploadResource::BulkMutationVariables,
            http_method: "POST".to_string(),
            file_size: None,
        };

        let data: StagedUploadsData = self
            .graphql(
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
                &json!({ "input": [input] }),
            )
            .await?;

        if !data.staged_uploads_create.user_errors.is_empty() {
            return Err(ShopifyAPIError::Other(format!(
                "stagedUploadsCreate returned errors: {:?}",
                data.staged_uploads_create.user_errors
            )));
        }

        data.staged_uploads_create
            .staged_targets
            .into_iter()
            .next()
            .ok_or_else(|| ShopifyAPIError::Other("no staged upload target returned".to_string()))
    }
}

pub fn parse_jsonl<T>(body: &str) -> Result<Vec<T>, ShopifyAPIError>
where
    T: serde::de::DeserializeOwned,
{
    body.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).map_err(ShopifyAPIError::JsonParseError))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonl_bulk_results() {
        let parsed: Vec<serde_json::Value> = parse_jsonl("{\"id\":1}\n{\"id\":2}\n\n").unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["id"], 1);
    }
}
