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
//! [`Shopify::bulk_query_rows`] holds every row in memory. For an export
//! of gigabytes, [`Shopify::bulk_query_stream`] reads the rows as the file
//! downloads ([`BulkRows`]), and [`Shopify::bulk_query_grouped`] each
//! top-level object with its nested objects ([`BulkGroups`]). Neither holds
//! the file: [`BulkRows`] holds the line being read and the rest of the
//! chunk it ends in, in a buffer sized for the longest recent line; [`BulkGroups`] holds that, and the group being read,
//! its top-level object with every child read so far and their IDs. A
//! product with 2,000 variants is held with its 2,000 variants.
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
use crate::{Error, Page, Result, Shopify, UserError};
use shopify_api_macros::{__graphql_internal as graphql, __input_internal as input};

mod rows;

pub use rows::{BulkGroups, BulkRows, DEFAULT_MAX_LINE_LENGTH};

/// How long a download waits for the answer, then for each chunk of it; a
/// transport that does not stream gives it the whole download.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// What an [`Error::Http`] of a download keeps of the body: its first MiB.
const ERROR_BODY_LIMIT: usize = 1024 * 1024;

graphql! {
    mod run_query = file("graphql/operations/bulk_run_query.graphql");
    mod run_mutation = file("graphql/operations/bulk_run_mutation.graphql");
    mod get = file("graphql/operations/bulk_get.graphql");
    mod list = file("graphql/operations/bulk_list.graphql");
    mod cancel = file("graphql/operations/bulk_cancel.graphql");
    mod staged_uploads = file("graphql/operations/bulk_staged_uploads.graphql");
}

/// A bulk operation, as Shopify last reported it. `Debug` leaves out the
/// query string of `url` and `partial_data_url`: their signature.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
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

impl std::fmt::Debug for BulkOperation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let unsigned = |url: &Option<String>| url.as_deref().map(crate::error::without_query);
        formatter
            .debug_struct("BulkOperation")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("status", &self.status)
            .field("error_code", &self.error_code)
            .field("created_at", &self.created_at)
            .field("completed_at", &self.completed_at)
            .field("object_count", &self.object_count)
            .field("root_object_count", &self.root_object_count)
            .field("file_size", &self.file_size)
            .field("url", &unsigned(&self.url))
            .field("partial_data_url", &unsigned(&self.partial_data_url))
            .field("query", &self.query)
            .finish()
    }
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
    /// The status as an error shows it: an unknown one is not quoted.
    fn shown(&self) -> &'static str {
        match self {
            Self::Created => "Created",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Canceling => "Canceling",
            Self::Canceled => "Canceled",
            Self::Failed => "Failed",
            Self::Expired => "Expired",
            Self::Other(_) => "with a status this crate does not know",
        }
    }

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
                // The value is not shown: it is the answer's.
                .map_err(|_| Error::Bulk(format!("`{field}` is not an unsigned integer")))
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

/// The ID of a bulk operation as an error shows it: a `gid://` of Shopify,
/// else nothing of the text.
fn shown_id(id: &str) -> &str {
    let shown = id
        .strip_prefix("gid://shopify/BulkOperation/")
        .is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        });
    if shown { id } else { "[id not shown]" }
}

/// The error of an operation that ended without completing.
pub(crate) fn not_completed(operation: &BulkOperation) -> Error {
    Error::Bulk(format!(
        "the bulk operation {} ended {}{}",
        shown_id(&operation.id),
        operation.status.shown(),
        operation
            .error_code
            .as_deref()
            .map(|code| format!(" ({})", shown_error_code(code)))
            .unwrap_or_default()
    ))
}

/// The error of a wait that ran out.
pub(crate) fn timed_out(id: &str, timeout: Duration) -> Error {
    Error::Timeout(format!(
        "the bulk operation {} did not end within {timeout:?}",
        shown_id(id)
    ))
}

/// A `BulkOperationErrorCode` as an error shows it.
fn shown_error_code(code: &str) -> &'static str {
    match code {
        "ACCESS_DENIED" => "ACCESS_DENIED",
        "INTERNAL_SERVER_ERROR" => "INTERNAL_SERVER_ERROR",
        "TIMEOUT" => "TIMEOUT",
        _ => "an error code this crate does not know",
    }
}

/// Reads the `BulkOperationFields` of a generated type.
pub(crate) fn operation_of(selection: &impl Serialize) -> Result<BulkOperation> {
    let wire: Wire = crate::decode::reserialize(selection)?;
    wire.into_operation()
}

/// The user errors of a generated payload.
fn user_errors(errors: &impl Serialize) -> Result<Vec<UserError>> {
    crate::decode::reserialize(errors)
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
            .execute(run_query::Variables {
                query: query.to_string(),
                group_objects,
            })
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
    /// The upload target is reserved first: an item that does not
    /// serialize fails after it, and the target goes unused.
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
        self.run_bulk_mutation_iter(mutation, variables).await
    }

    /// [`Shopify::run_bulk_mutation`] over the items of an iterator, each
    /// serialized as it is taken: the items need not all be in memory, only
    /// the JSONL file, which Shopify caps at 100 MB.
    ///
    /// The upload target is reserved first: an item that does not
    /// serialize fails after it, and the target goes unused.
    ///
    /// ```no_run
    /// # async fn run(shopify: shopify_api::Shopify) -> shopify_api::Result<()> {
    /// let rows = (1..=50_000).map(|id| {
    ///     serde_json::json!({ "product": { "id": format!("gid://shopify/Product/{id}"), "status": "ARCHIVED" } })
    /// });
    /// let operation = shopify
    ///     .run_bulk_mutation_iter(
    ///         "mutation($product: ProductUpdateInput!) { productUpdate(product: $product) { userErrors { message } } }",
    ///         rows,
    ///     )
    ///     .await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn run_bulk_mutation_iter<I>(
        &self,
        mutation: &str,
        variables: I,
    ) -> Result<BulkOperation>
    where
        I: IntoIterator,
        I::Item: Serialize,
    {
        let path = self.stage_bulk_variables_iter(variables).await?;
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
            .execute(run_mutation::Variables {
                mutation: mutation.to_string(),
                staged_upload_path: staged_upload_path.to_string(),
                client_identifier: client_identifier.map(str::to_string),
            })
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
    ///
    /// The upload target is reserved first: an item that does not
    /// serialize fails after it, and the target goes unused.
    pub async fn stage_bulk_variables<T: Serialize>(&self, variables: &[T]) -> Result<String> {
        self.stage_bulk_variables_iter(variables).await
    }

    /// [`Shopify::stage_bulk_variables`] over the items of an iterator,
    /// written straight into the body of the upload as they are taken.
    ///
    /// The upload target is reserved first: an item that does not
    /// serialize fails after it, and the target goes unused.
    pub async fn stage_bulk_variables_iter<I>(&self, variables: I) -> Result<String>
    where
        I: IntoIterator,
        I::Item: Serialize,
    {
        let data = self
            .execute(staged_uploads::Variables {
                input: vec![input!(StagedUploadInput {
                    filename: "bulk_op_vars.jsonl",
                    mimeType: "text/jsonl",
                    resource: BULK_MUTATION_VARIABLES,
                    httpMethod: POST,
                })],
            })
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

        // The storage server wants the length of the body up front: the
        // file is built in memory, once, in the body itself.
        let (content_type, body) =
            multipart(&parameters, "bulk_op_vars.jsonl", "text/jsonl", |body| {
                for item in variables {
                    serde_json::to_writer(&mut *body, &item)
                        .map_err(|error| Error::variables(None, &error))?;
                    body.push(b'\n');
                }
                Ok(())
            })?;
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
        let data = self.execute(get::Variables { id: id.to_string() }).await?;
        data.bulk_operation.as_ref().map(operation_of).transpose()
    }

    /// One page of the shop's bulk operations, newest first.
    pub async fn bulk_operations(
        &self,
        filter: BulkOperationsFilter,
    ) -> Result<Page<BulkOperation>> {
        let data = self
            .execute(list::Variables {
                first: i64::from(filter.first),
                after: filter.after,
                query: filter.query,
                reverse: Some(true),
            })
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
            .execute(cancel::Variables { id: id.to_string() })
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
                    return Err(timed_out(id, timeout));
                }
                wait = wait.min(timeout - elapsed);
            }
            tokio::time::sleep(wait).await;
            let operation = self.bulk_operation(id).await?.ok_or_else(|| {
                Error::Bulk(format!(
                    "the bulk operation {} does not exist",
                    shown_id(id)
                ))
            })?;
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
    /// The rows are collected in a `Vec`, up to the first error: for an
    /// export too large for memory, read them one by one with
    /// [`Shopify::bulk_query_stream`].
    ///
    /// A nested connection writes its objects on lines of their own, with
    /// the `__parentId` of their parent: read the lines as `serde_json::Value`,
    /// or as an enum of your types, to put them back together, or use
    /// [`Shopify::bulk_query_grouped`].
    pub async fn bulk_query_rows<T: DeserializeOwned>(
        &self,
        query: &str,
        options: BulkWaitOptions,
    ) -> Result<Vec<T>> {
        self.bulk_query_stream(query, options)
            .await?
            .collect()
            .await
    }

    /// Runs a bulk query to its end, then reads its results line by line
    /// as they download: the memory it takes follows the lines being read,
    /// not the file (see [`BulkRows`]). Fails when the operation does not complete.
    ///
    /// The stream ends after some errors and goes on after others: see
    /// [`BulkRows`].
    ///
    /// ```no_run
    /// use serde::Deserialize;
    /// use shopify_api::bulk::BulkWaitOptions;
    ///
    /// #[derive(Deserialize)]
    /// struct Product {
    ///     id: String,
    ///     title: String,
    /// }
    ///
    /// # async fn run(shopify: shopify_api::Shopify) -> shopify_api::Result<()> {
    /// let mut products = shopify
    ///     .bulk_query_stream::<Product>(
    ///         "{ products { edges { node { id title } } } }",
    ///         BulkWaitOptions::default(),
    ///     )
    ///     .await?;
    /// while let Some(product) = products.next_row().await {
    ///     let product = product?;
    ///     println!("{} {}", product.id, product.title);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_query_stream<T: DeserializeOwned>(
        &self,
        query: &str,
        options: BulkWaitOptions,
    ) -> Result<BulkRows<T>> {
        let operation = self.run_bulk_query(query).await?;
        match self.completed_url(&operation.id, options).await? {
            Some(url) => self.download_bulk_results_stream(&url).await,
            None => Ok(BulkRows::new(None, "")),
        }
    }

    /// Runs a bulk query with `groupObjects` to its end, then reads its
    /// results one top-level object at a time, with the objects of its
    /// nested connections, as they download. Fails when the operation does
    /// not complete.
    ///
    /// Grouping makes Shopify slower, and more likely to time out: use it
    /// when you need the children with their parent. Every object that has
    /// a nested connection must select `id`, without an alias: Shopify
    /// refuses the query otherwise. See [`BulkGroups`] for the order it
    /// relies on and connections two levels deep.
    ///
    /// ```no_run
    /// use serde::Deserialize;
    /// use shopify_api::bulk::BulkWaitOptions;
    ///
    /// #[derive(Deserialize)]
    /// struct Product {
    ///     id: String,
    ///     title: String,
    /// }
    ///
    /// #[derive(Deserialize)]
    /// struct Variant {
    ///     sku: Option<String>,
    /// }
    ///
    /// # async fn run(shopify: shopify_api::Shopify) -> shopify_api::Result<()> {
    /// let mut products = shopify
    ///     .bulk_query_grouped::<Product, Variant>(
    ///         "{ products { edges { node { id title variants { edges { node { sku } } } } } } }",
    ///         BulkWaitOptions::default(),
    ///     )
    ///     .await?;
    /// while let Some(group) = products.next_group().await {
    ///     let (product, variants) = group?;
    ///     println!("{} has {} variants", product.title, variants.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_query_grouped<P: DeserializeOwned, C: DeserializeOwned>(
        &self,
        query: &str,
        options: BulkWaitOptions,
    ) -> Result<BulkGroups<P, C>> {
        let operation = self.run_bulk_query_grouped(query, true).await?;
        match self.completed_url(&operation.id, options).await? {
            Some(url) => Ok(self.download_bulk_results_grouped(&url).await?.grouped()),
            None => Ok(BulkGroups::new(None, "").grouped()),
        }
    }

    /// Waits for an operation, and returns the address of its results:
    /// `None` when it found nothing.
    async fn completed_url(&self, id: &str, options: BulkWaitOptions) -> Result<Option<String>> {
        let operation = self.wait_for_bulk_operation(id, options).await?;
        if operation.status != BulkOperationStatus::Completed {
            return Err(not_completed(&operation));
        }
        Ok(operation.url)
    }

    /// Downloads the results of a bulk operation and reads each line as
    /// `T`, up to the first error.
    ///
    /// The rows are collected in a `Vec`: for an export too large for
    /// memory, use [`Shopify::download_bulk_results_stream`]. The address
    /// is signed: errors leave its query string out.
    pub async fn download_bulk_results<T: DeserializeOwned>(&self, url: &str) -> Result<Vec<T>> {
        self.download_bulk_results_stream(url)
            .await?
            .collect()
            .await
    }

    /// Downloads the results of a bulk operation, its `url` or its
    /// `partial_data_url`, and reads each line as `T` as it arrives. See
    /// [`BulkRows`].
    ///
    /// It returns once the answer's headers arrive. An answer that is not a
    /// success is an [`Error::Http`] with the first MiB of its body. The
    /// address is signed: errors leave its query string out.
    ///
    /// The default transport waits 15 minutes at most for the answer, then
    /// for each chunk, however long the whole download takes: read the rows
    /// as they come, a storage server may close a connection left idle.
    pub async fn download_bulk_results_stream<T: DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<BulkRows<T>> {
        let body = self.open_bulk_results(url).await?;
        Ok(BulkRows::new(Some(body), url))
    }

    /// [`Shopify::download_bulk_results_stream`], one top-level object and
    /// its children at a time. The operation must have run with
    /// `groupObjects`, and every object of it that has a nested connection
    /// must have an `id`, not aliased: see [`BulkGroups`].
    pub async fn download_bulk_results_grouped<P: DeserializeOwned, C: DeserializeOwned>(
        &self,
        url: &str,
    ) -> Result<BulkGroups<P, C>> {
        let body = self.open_bulk_results(url).await?;
        Ok(BulkGroups::new(Some(body), url))
    }

    async fn open_bulk_results(&self, url: &str) -> Result<crate::transport::BodyStream> {
        let request = HttpRequest::new(Method::Get, url).with_timeout(DOWNLOAD_TIMEOUT);
        let response = self
            .shared
            .endpoint
            .transport
            .send_streaming(request)
            .await
            .map_err(|source| Error::transport(url, source))?;
        if !response.is_success() {
            let response = response.into_response(ERROR_BODY_LIMIT).await;
            return Err(Error::http(url, &response));
        }
        Ok(response.body)
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

/// Reads each non-empty line of a JSONL file already in memory as `T`. To
/// read one as it downloads, see [`BulkRows`].
pub fn parse_jsonl<T: DeserializeOwned>(body: &[u8]) -> Result<Vec<T>> {
    body.split(|byte| *byte == b'\n')
        .enumerate()
        .filter(|(_, line)| !line.iter().all(u8::is_ascii_whitespace))
        .map(|(index, line)| crate::decode::from_jsonl_line(line, index + 1))
        .collect()
}

/// A `multipart/form-data` body: the fields, then the file `write_file`
/// writes. Returns its `Content-Type` and its bytes.
fn multipart(
    fields: &[(String, String)],
    filename: &str,
    mime: &str,
    write_file: impl FnOnce(&mut Vec<u8>) -> Result<()>,
) -> Result<(String, Vec<u8>)> {
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
    write_file(&mut body)?;
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Ok((format!("multipart/form-data; boundary={boundary}"), body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_hides_the_signature_of_the_download_addresses() {
        let operation = BulkOperation {
            id: "gid://shopify/BulkOperation/1".to_string(),
            kind: BulkOperationType::Query,
            status: BulkOperationStatus::Completed,
            error_code: None,
            created_at: "2026-10-22T10:00:00Z".to_string(),
            completed_at: None,
            object_count: 0,
            root_object_count: 0,
            file_size: None,
            url: Some("https://storage.googleapis.com/a.jsonl?X-Goog-Signature=SECRET".to_string()),
            partial_data_url: Some(
                "https://storage.googleapis.com/b.jsonl?X-Goog-Signature=SECRET".to_string(),
            ),
            query: "{ shop { name } }".to_string(),
        };
        let shown = format!("{operation:?}");
        assert!(!shown.contains("SECRET"), "{shown}");
        assert!(
            shown.contains("https://storage.googleapis.com/a.jsonl"),
            "{shown}"
        );
        assert!(
            shown.contains("https://storage.googleapis.com/b.jsonl"),
            "{shown}"
        );
    }

    #[test]
    fn parses_jsonl_and_names_the_line_at_fault() {
        let rows: Vec<serde_json::Value> = parse_jsonl(b"{\"id\":1}\n\n{\"id\":2}\n").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1]["id"], 2);

        #[derive(Debug, Deserialize)]
        #[allow(dead_code, reason = "only decoded: the test checks the decoding")]
        struct Row {
            id: String,
        }
        let error = parse_jsonl::<Row>(b"{\"id\":\"a\"}\n{\"id\":2}\n").unwrap_err();
        assert_eq!(
            error.to_string(),
            "Shopify's answer does not match the operation at `id`: invalid type: integer, expected a string at line 2 column 7"
        );
    }

    #[test]
    fn builds_a_multipart_body() {
        let fields = vec![("key".to_string(), "tmp/1/bulk".to_string())];
        let (content_type, body) = multipart(&fields, "vars.jsonl", "text/jsonl", |body| {
            body.extend_from_slice(b"{}\n");
            Ok(())
        })
        .unwrap();
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
