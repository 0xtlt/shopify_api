//! Bulk operations: a query over a whole catalog, or a mutation over
//! thousands of inputs, run by Shopify in the background, outside the rate limit.
//!
//! ```no_run
//! use serde::Deserialize;
//! use shopify_api::bulk::BulkWaitOptions;
//!
//! #[derive(Deserialize)]
//! struct Product {
//!     id: String,
//!     title: String,
//! }
//!
//! # async fn run(shopify: shopify_api::Shopify) -> shopify_api::Result<()> {
//! // Start, wait for the end, download and read every line.
//! let products: Vec<Product> = shopify
//!     .bulk_query_rows("{ products { edges { node { id title } } } }", BulkWaitOptions::default())
//!     .await?;
//! # Ok(())
//! # }
//! ```
//!
//! The query of a bulk operation is a plain string. To have it checked when
//! the crate compiles, declare it with [`graphql!`](crate::graphql!) and pass
//! its `QUERY`:
//!
//! ```no_run
//! shopify_api::graphql! {
//!     mod all_products = "{ products { edges { node { id title } } } }";
//! }
//!
//! # async fn run(shopify: shopify_api::Shopify) -> shopify_api::Result<()> {
//! let operation = shopify.run_bulk_query(all_products::QUERY).await?;
//! # Ok(())
//! # }
//! ```

use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::transport::{HttpRequest, Method};
use crate::{Error, Page, Result, Shopify, UserError, graphql};

/// How long a download of results may take.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

graphql! {
    mod run_query = "#graphql
        mutation BulkOperationRunQuery($query: String!, $groupObjects: Boolean!) {
          bulkOperationRunQuery(query: $query, groupObjects: $groupObjects) {
            bulkOperation { ...BulkOperationFields }
            userErrors { field message code }
          }
        }
        fragment BulkOperationFields on BulkOperation {
          id type status errorCode createdAt completedAt objectCount rootObjectCount fileSize url partialDataUrl query
        }";
}

graphql! {
    mod run_mutation = "#graphql
        mutation BulkOperationRunMutation($mutation: String!, $stagedUploadPath: String!, $clientIdentifier: String) {
          bulkOperationRunMutation(mutation: $mutation, stagedUploadPath: $stagedUploadPath, clientIdentifier: $clientIdentifier) {
            bulkOperation { ...BulkOperationFields }
            userErrors { field message code }
          }
        }
        fragment BulkOperationFields on BulkOperation {
          id type status errorCode createdAt completedAt objectCount rootObjectCount fileSize url partialDataUrl query
        }";
}

graphql! {
    mod get = "#graphql
        query BulkOperation($id: ID!) {
          bulkOperation(id: $id) { ...BulkOperationFields }
        }
        fragment BulkOperationFields on BulkOperation {
          id type status errorCode createdAt completedAt objectCount rootObjectCount fileSize url partialDataUrl query
        }";
}

graphql! {
    mod list = "#graphql
        query BulkOperations($first: Int!, $after: String, $query: String, $reverse: Boolean) {
          bulkOperations(first: $first, after: $after, query: $query, reverse: $reverse, sortKey: CREATED_AT) {
            nodes { ...BulkOperationFields }
            pageInfo { hasNextPage endCursor }
          }
        }
        fragment BulkOperationFields on BulkOperation {
          id type status errorCode createdAt completedAt objectCount rootObjectCount fileSize url partialDataUrl query
        }";
}

graphql! {
    mod cancel = "#graphql
        mutation BulkOperationCancel($id: ID!) {
          bulkOperationCancel(id: $id) {
            bulkOperation { ...BulkOperationFields }
            userErrors { field message }
          }
        }
        fragment BulkOperationFields on BulkOperation {
          id type status errorCode createdAt completedAt objectCount rootObjectCount fileSize url partialDataUrl query
        }";
}

graphql! {
    mod staged_uploads = "#graphql
        mutation StagedUploadsCreate($input: [StagedUploadInput!]!) {
          stagedUploadsCreate(input: $input) {
            stagedTargets { url resourceUrl parameters { name value } }
            userErrors { field message }
          }
        }";
}

/// A bulk operation, as Shopify last reported it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct BulkOperation {
    pub id: String,
    pub kind: BulkOperationType,
    pub status: BulkOperationStatus,
    /// Why it failed: `ACCESS_DENIED`, `INTERNAL_SERVER_ERROR`, `TIMEOUT`.
    pub error_code: Option<String>,
    pub created_at: String,
    pub completed_at: Option<String>,
    /// The objects written so far, nested ones included.
    pub object_count: u64,
    /// The top-level objects written so far.
    pub root_object_count: u64,
    /// The size of the result file, in bytes.
    pub file_size: Option<u64>,
    /// Where to download the results, once completed. It expires a week later.
    pub url: Option<String>,
    /// Where to download what was written before a failure.
    pub partial_data_url: Option<String>,
    pub query: String,
}

/// The status of a bulk operation. A status Shopify adds later is `Other`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BulkOperationStatus {
    Created,
    Running,
    Completed,
    Canceling,
    Canceled,
    Failed,
    Expired,
    Other(String),
}

impl BulkOperationStatus {
    fn parse(value: &str) -> Self {
        match value {
            "CREATED" => Self::Created,
            "RUNNING" => Self::Running,
            "COMPLETED" => Self::Completed,
            "CANCELING" => Self::Canceling,
            "CANCELED" => Self::Canceled,
            "FAILED" => Self::Failed,
            "EXPIRED" => Self::Expired,
            other => Self::Other(other.to_string()),
        }
    }

    /// It will not change anymore.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Canceled | Self::Failed | Self::Expired
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BulkOperationType {
    Query,
    Mutation,
    Other(String),
}

impl BulkOperationType {
    fn parse(value: &str) -> Self {
        match value {
            "QUERY" => Self::Query,
            "MUTATION" => Self::Mutation,
            other => Self::Other(other.to_string()),
        }
    }
}

/// The selection every operation of this module makes on `BulkOperation`,
/// as Shopify sends it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Wire {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    status: String,
    error_code: Option<String>,
    created_at: String,
    completed_at: Option<String>,
    object_count: String,
    root_object_count: String,
    file_size: Option<String>,
    url: Option<String>,
    partial_data_url: Option<String>,
    query: String,
}

impl Wire {
    fn into_operation(self) -> Result<BulkOperation> {
        let number = |field: &str, value: &str| {
            value
                .parse::<u64>()
                .map_err(|_| Error::Bulk(format!("`{field}` is not a number: {value}")))
        };
        Ok(BulkOperation {
            object_count: number("objectCount", &self.object_count)?,
            root_object_count: number("rootObjectCount", &self.root_object_count)?,
            file_size: self
                .file_size
                .as_deref()
                .map(|size| number("fileSize", size))
                .transpose()?,
            id: self.id,
            kind: BulkOperationType::parse(&self.kind),
            status: BulkOperationStatus::parse(&self.status),
            error_code: self.error_code,
            created_at: self.created_at,
            completed_at: self.completed_at,
            url: self.url,
            partial_data_url: self.partial_data_url,
            query: self.query,
        })
    }
}

/// Reads the `BulkOperationFields` of a generated type.
fn operation_of(selection: &impl Serialize) -> Result<BulkOperation> {
    let wire: Wire = crate::graphql::decode(serde_json::to_value(selection)?)?;
    wire.into_operation()
}

/// The user errors of a generated payload.
fn user_errors(errors: &impl Serialize) -> Result<Vec<UserError>> {
    crate::graphql::decode(serde_json::to_value(errors)?)
}

/// How [`Shopify::wait_for_bulk_operation`] polls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BulkWaitOptions {
    first_poll: Duration,
    poll_interval: Duration,
    timeout: Option<Duration>,
}

impl BulkWaitOptions {
    /// The first poll a second after the start, then twice as late each
    /// time, up to every ten seconds; no timeout.
    pub const fn new() -> Self {
        Self {
            first_poll: Duration::from_secs(1),
            poll_interval: Duration::from_secs(10),
            timeout: None,
        }
    }

    /// The longest wait between two polls.
    pub const fn poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        if self.first_poll.as_nanos() > interval.as_nanos() {
            self.first_poll = interval;
        }
        self
    }

    /// Gives up after `timeout`, with [`Error::Timeout`]. The operation keeps running.
    pub const fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }
}

impl Default for BulkWaitOptions {
    fn default() -> Self {
        Self::new()
    }
}

/// Which bulk operations [`Shopify::bulk_operations`] lists, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BulkOperationsFilter {
    first: u32,
    after: Option<String>,
    query: Option<String>,
}

impl BulkOperationsFilter {
    /// The ten newest operations.
    pub fn new() -> Self {
        Self {
            first: 10,
            after: None,
            query: None,
        }
    }

    /// How many per page, up to 250.
    pub fn first(mut self, first: u32) -> Self {
        self.first = first.clamp(1, 250);
        self
    }

    /// The page after this cursor.
    pub fn after(mut self, cursor: impl Into<String>) -> Self {
        self.after = Some(cursor.into());
        self
    }

    /// A search, such as `status:RUNNING` or `operation_type:QUERY`.
    pub fn query(mut self, query: impl Into<String>) -> Self {
        self.query = Some(query.into());
        self
    }
}

impl Default for BulkOperationsFilter {
    fn default() -> Self {
        Self::new()
    }
}

impl Shopify {
    /// Starts a bulk query. Shopify refuses a query that is not a bulk
    /// query (one connection at the root, no `first`) with user errors,
    /// returned as [`Error::UserErrors`].
    pub async fn run_bulk_query(&self, query: &str) -> Result<BulkOperation> {
        self.run_bulk_query_grouped(query, false).await
    }

    /// [`Shopify::run_bulk_query`], with nested objects written right after
    /// their parent (`groupObjects`), which is slower.
    pub async fn run_bulk_query_grouped(
        &self,
        query: &str,
        group_objects: bool,
    ) -> Result<BulkOperation> {
        let data = self
            .execute(run_query::operation(run_query::Variables {
                query: query.to_string(),
                group_objects,
            }))
            .await?;
        let payload = data
            .bulk_operation_run_query
            .ok_or_else(|| Error::Bulk("bulkOperationRunQuery returned nothing".to_string()))?;
        started(
            "bulkOperationRunQuery",
            payload.bulk_operation,
            &payload.user_errors,
        )
    }

    /// Starts a bulk mutation over `variables`: one run of `mutation` per
    /// item, which holds the variables of that run. The items are uploaded
    /// as a JSONL file first.
    ///
    /// ```no_run
    /// # async fn run(shopify: shopify_api::Shopify) -> shopify_api::Result<()> {
    /// let rows = vec![
    ///     serde_json::json!({ "product": { "id": "gid://shopify/Product/1", "title": "New title" } }),
    ///     serde_json::json!({ "product": { "id": "gid://shopify/Product/2", "title": "Other title" } }),
    /// ];
    /// let operation = shopify
    ///     .run_bulk_mutation(
    ///         "mutation($product: ProductUpdateInput!) { productUpdate(product: $product) { userErrors { message } } }",
    ///         &rows,
    ///     )
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn run_bulk_mutation<T: Serialize>(
        &self,
        mutation: &str,
        variables: &[T],
    ) -> Result<BulkOperation> {
        let path = self.stage_bulk_variables(variables).await?;
        self.run_bulk_mutation_staged(mutation, &path, None).await
    }

    /// Starts a bulk mutation over a JSONL file already uploaded with
    /// [`Shopify::stage_bulk_variables`]. `client_identifier` is yours, kept
    /// with the operation.
    pub async fn run_bulk_mutation_staged(
        &self,
        mutation: &str,
        staged_upload_path: &str,
        client_identifier: Option<&str>,
    ) -> Result<BulkOperation> {
        let data = self
            .execute(run_mutation::operation(run_mutation::Variables {
                mutation: mutation.to_string(),
                staged_upload_path: staged_upload_path.to_string(),
                client_identifier: client_identifier.map(str::to_string),
            }))
            .await?;
        let payload = data
            .bulk_operation_run_mutation
            .ok_or_else(|| Error::Bulk("bulkOperationRunMutation returned nothing".to_string()))?;
        started(
            "bulkOperationRunMutation",
            payload.bulk_operation,
            &payload.user_errors,
        )
    }

    /// Uploads `variables` as the JSONL file of a bulk mutation, one item per
    /// line, and returns its staged upload path.
    pub async fn stage_bulk_variables<T: Serialize>(&self, variables: &[T]) -> Result<String> {
        let mut jsonl = Vec::new();
        for item in variables {
            serde_json::to_writer(&mut jsonl, item)?;
            jsonl.push(b'\n');
        }

        let data = self
            .execute(staged_uploads::operation(staged_uploads::Variables {
                input: vec![staged_uploads::StagedUploadInput {
                    filename: "bulk_op_vars.jsonl".to_string(),
                    mime_type: "text/jsonl".to_string(),
                    resource: "BULK_MUTATION_VARIABLES".to_string(),
                    http_method: Some("POST".to_string()),
                    ..Default::default()
                }],
            }))
            .await?;
        let payload = data
            .staged_uploads_create
            .ok_or_else(|| Error::Bulk("stagedUploadsCreate returned nothing".to_string()))?;
        let errors = user_errors(&payload.user_errors)?;
        if !errors.is_empty() {
            return Err(Error::UserErrors {
                mutation: "stagedUploadsCreate",
                errors,
            });
        }
        let target = payload
            .staged_targets
            .into_iter()
            .flatten()
            .next()
            .ok_or_else(|| Error::Bulk("stagedUploadsCreate returned no target".to_string()))?;
        let url = target
            .url
            .ok_or_else(|| Error::Bulk("the staged upload target has no address".to_string()))?;
        let parameters: Vec<(String, String)> = target
            .parameters
            .into_iter()
            .map(|parameter| (parameter.name, parameter.value))
            .collect();
        let key = parameters
            .iter()
            .find(|(name, _)| name == "key")
            .map(|(_, value)| value.clone())
            .ok_or_else(|| {
                Error::Bulk("the staged upload target has no `key` parameter".to_string())
            })?;

        let (content_type, body) =
            multipart(&parameters, "bulk_op_vars.jsonl", "text/jsonl", &jsonl);
        let request = HttpRequest::new(Method::Post, &url)
            .with_header("Content-Type", content_type)
            .with_body(body)
            .with_timeout(DOWNLOAD_TIMEOUT);
        let response = self
            .shared
            .endpoint
            .transport
            .send(request)
            .await
            .map_err(|source| Error::transport(&url, source))?;
        if !response.is_success() {
            return Err(Error::http(&url, &response));
        }
        Ok(key)
    }

    /// A bulk operation by ID, `None` when the shop has none of that ID.
    pub async fn bulk_operation(&self, id: &str) -> Result<Option<BulkOperation>> {
        let data = self
            .execute(get::operation(get::Variables { id: id.to_string() }))
            .await?;
        data.bulk_operation.as_ref().map(operation_of).transpose()
    }

    /// One page of the shop's bulk operations, newest first.
    pub async fn bulk_operations(
        &self,
        filter: BulkOperationsFilter,
    ) -> Result<Page<BulkOperation>> {
        let data = self
            .execute(list::operation(list::Variables {
                first: i64::from(filter.first),
                after: filter.after,
                query: filter.query,
                reverse: Some(true),
            }))
            .await?;
        let connection = data.bulk_operations;
        let items = connection
            .nodes
            .iter()
            .map(operation_of)
            .collect::<Result<_>>()?;
        Ok(Page::new(
            items,
            connection.page_info.has_next_page,
            connection.page_info.end_cursor,
        ))
    }

    /// Asks Shopify to cancel a running bulk operation.
    pub async fn cancel_bulk_operation(&self, id: &str) -> Result<BulkOperation> {
        let data = self
            .execute(cancel::operation(cancel::Variables { id: id.to_string() }))
            .await?;
        let payload = data
            .bulk_operation_cancel
            .ok_or_else(|| Error::Bulk("bulkOperationCancel returned nothing".to_string()))?;
        started(
            "bulkOperationCancel",
            payload.bulk_operation,
            &payload.user_errors,
        )
    }

    /// Polls a bulk operation until it completes, fails, is canceled or expires.
    pub async fn wait_for_bulk_operation(
        &self,
        id: &str,
        options: BulkWaitOptions,
    ) -> Result<BulkOperation> {
        let started = Instant::now();
        let mut wait = options.first_poll;
        loop {
            if let Some(timeout) = options.timeout {
                let elapsed = started.elapsed();
                if elapsed >= timeout {
                    return Err(Error::Timeout(format!(
                        "the bulk operation {id} did not end within {timeout:?}"
                    )));
                }
                wait = wait.min(timeout - elapsed);
            }
            tokio::time::sleep(wait).await;
            let operation = self
                .bulk_operation(id)
                .await?
                .ok_or_else(|| Error::Bulk(format!("the bulk operation {id} does not exist")))?;
            // A status this version does not know ends the wait too: it is
            // the caller's to read, rather than a loop that never ends.
            if operation.status.is_terminal()
                || matches!(operation.status, BulkOperationStatus::Other(_))
            {
                return Ok(operation);
            }
            wait = wait.saturating_mul(2).min(options.poll_interval);
        }
    }

    /// Runs a bulk query to its end and reads every line of its results as
    /// `T`. Fails when the operation does not complete.
    ///
    /// A nested connection writes its objects on lines of their own, with
    /// the `__parentId` of their parent: read the lines as `serde_json::Value`,
    /// or as an enum of your types, to put them back together.
    pub async fn bulk_query_rows<T: DeserializeOwned>(
        &self,
        query: &str,
        options: BulkWaitOptions,
    ) -> Result<Vec<T>> {
        let operation = self.run_bulk_query(query).await?;
        let operation = self.wait_for_bulk_operation(&operation.id, options).await?;
        if operation.status != BulkOperationStatus::Completed {
            return Err(Error::Bulk(format!(
                "the bulk operation {} ended {:?}{}",
                operation.id,
                operation.status,
                operation
                    .error_code
                    .map(|code| format!(" ({code})"))
                    .unwrap_or_default()
            )));
        }
        match &operation.url {
            Some(url) => self.download_bulk_results(url).await,
            // An operation that found nothing has no file.
            None => Ok(Vec::new()),
        }
    }

    /// Downloads the results of a bulk operation and reads each line as `T`.
    ///
    /// The file is held in memory while it is read: for an export of
    /// gigabytes, download `url` with a client of your own and read it with
    /// [`parse_jsonl`] chunk by chunk. The address is signed: errors leave
    /// its query string out.
    pub async fn download_bulk_results<T: DeserializeOwned>(&self, url: &str) -> Result<Vec<T>> {
        let request = HttpRequest::new(Method::Get, url).with_timeout(DOWNLOAD_TIMEOUT);
        let response = self
            .shared
            .endpoint
            .transport
            .send(request)
            .await
            .map_err(|source| Error::transport(url, source))?;
        if !response.is_success() {
            return Err(Error::http(url, &response));
        }
        parse_jsonl(&response.body)
    }
}

/// The operation a bulk mutation started, or the user errors that refused it.
fn started<O: Serialize, E: Serialize>(
    mutation: &'static str,
    operation: Option<O>,
    errors: &E,
) -> Result<BulkOperation> {
    let errors = user_errors(errors)?;
    if !errors.is_empty() {
        return Err(Error::UserErrors { mutation, errors });
    }
    let operation =
        operation.ok_or_else(|| Error::Bulk(format!("{mutation} returned no operation")))?;
    operation_of(&operation)
}

/// Reads each non-empty line of a JSONL file as `T`.
pub fn parse_jsonl<T: DeserializeOwned>(body: &[u8]) -> Result<Vec<T>> {
    body.split(|byte| *byte == b'\n')
        .enumerate()
        .filter(|(_, line)| !line.iter().all(u8::is_ascii_whitespace))
        .map(|(index, line)| {
            let mut deserializer = serde_json::Deserializer::from_slice(line);
            serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
                // The position serde_json adds is within the line, not the file.
                let inner = error.inner();
                let position = format!(" at line {} column {}", inner.line(), inner.column());
                let message = inner.to_string();
                Error::Decode {
                    path: format!("line {}: {}", index + 1, error.path()),
                    message: message
                        .strip_suffix(&position)
                        .unwrap_or(&message)
                        .to_string(),
                }
            })
        })
        .collect()
}

/// A `multipart/form-data` body: the fields, then the file. Returns its
/// `Content-Type` and its bytes.
fn multipart(
    fields: &[(String, String)],
    filename: &str,
    mime: &str,
    file: &[u8],
) -> (String, Vec<u8>) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();
    let boundary = format!("shopify-api-{nanos:x}");
    let quoted = |text: &str| text.replace('\\', "\\\\").replace('"', "\\\"");
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"\r\n\r\n{value}\r\n",
                quoted(name)
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{}\"\r\nContent-Type: {mime}\r\n\r\n",
            quoted(filename)
        )
        .as_bytes(),
    );
    body.extend_from_slice(file);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonl_and_names_the_line_at_fault() {
        let rows: Vec<serde_json::Value> = parse_jsonl(b"{\"id\":1}\n\n{\"id\":2}\n").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1]["id"], 2);

        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        struct Row {
            id: String,
        }
        let error = parse_jsonl::<Row>(b"{\"id\":\"a\"}\n{\"id\":2}\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "Shopify's answer does not match the operation at `line 2: id`: invalid type: integer `2`, expected a string"
        );
    }

    #[test]
    fn builds_a_multipart_body() {
        let fields = vec![("key".to_string(), "tmp/1/bulk".to_string())];
        let (content_type, body) = multipart(&fields, "vars.jsonl", "text/jsonl", b"{}\n");
        let boundary = content_type
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        let body = String::from_utf8(body).unwrap();
        assert_eq!(
            body,
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"key\"\r\n\r\ntmp/1/bulk\r\n\
                 --{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"vars.jsonl\"\r\nContent-Type: text/jsonl\r\n\r\n{{}}\n\r\n\
                 --{boundary}--\r\n"
            )
        );
    }

    #[test]
    fn statuses_say_when_they_end() {
        assert!(BulkOperationStatus::parse("COMPLETED").is_terminal());
        assert!(!BulkOperationStatus::parse("RUNNING").is_terminal());
        assert_eq!(
            BulkOperationStatus::parse("PAUSED"),
            BulkOperationStatus::Other("PAUSED".to_string())
        );
    }
}
