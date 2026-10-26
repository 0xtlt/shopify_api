//! Bulk operations declared with `graphql!(bulk(…))`: checked when the
//! crate compiles, their results read as the types it generates.

use std::fmt;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::value::RawValue;

use super::{
    BulkOperation, BulkRows, BulkWaitOptions, bulk_error, run_mutation, run_query, started,
};
use crate::error::is_graphql_name;
use crate::graphql::check::Invalid;
use crate::{
    Error, GraphqlError, IntoVariable, InvalidValueReason, Operation, OperationKind, Result,
    Shopify, VariableType,
};

pub use super::rows::BulkItems;

/// The text of a bulk query, with its variables apart. Not a public API.
#[doc(hidden)]
#[derive(Debug)]
pub struct BulkTemplate {
    pub parts: &'static [BulkPart],
    pub variables: &'static [BulkVariable],
}

#[doc(hidden)]
#[derive(Debug)]
pub enum BulkPart {
    Text(&'static str),
    /// `(name: value, …)`, where a value holds a variable.
    Arguments(&'static [BulkArgument]),
}

#[doc(hidden)]
#[derive(Debug)]
pub struct BulkArgument {
    pub name: &'static str,
    pub value: BulkValue,
}

#[doc(hidden)]
#[derive(Debug)]
pub enum BulkValue {
    /// A constant, as GraphQL text.
    Text(&'static str),
    /// The variable of this index in [`BulkTemplate::variables`].
    Variable(usize),
    List(&'static [BulkValue]),
    Object(&'static [BulkArgument]),
}

#[doc(hidden)]
#[derive(Debug)]
pub struct BulkVariable {
    pub name: &'static str,
    pub ty: BulkType,
    /// Its default value, as GraphQL text.
    pub default: Option<&'static str>,
}

/// Where the enum values of a variable are: they are written bare.
#[doc(hidden)]
#[derive(Debug)]
pub enum BulkType {
    /// No enum value.
    Plain,
    Enum,
    List(&'static BulkType),
    /// An input object: its fields that hold enum values.
    Input(&'static [(&'static str, BulkType)]),
}

/// A nested connection of a bulk query, and where its lines go.
#[doc(hidden)]
#[derive(Debug)]
pub struct BulkConnection {
    pub key: &'static str,
    /// The `__typename`s of its nodes.
    pub types: &'static [&'static str],
    pub connections: &'static [BulkConnection],
}

/// The type of a top-level object of a bulk query declared with
/// `graphql!(bulk(…))`, which `graphql!` implements: the item of
/// [`BulkItems`]. Its items are not a public API.
pub trait BulkItem: DeserializeOwned {
    /// The nested connections of the item, in the order of their slots.
    #[doc(hidden)]
    const CONNECTIONS: &'static [BulkConnection];

    /// Reads `line` as an object of the connection `slot` of the item, or,
    /// with `parent`, of the child `parent.1` of the connection `parent.0`.
    /// Returns its index in its list.
    #[doc(hidden)]
    fn __attach(
        &mut self,
        parent: Option<(usize, usize)>,
        slot: usize,
        line: &[u8],
        number: usize,
    ) -> Result<usize>;
}

/// What `BulkItem::__attach` pushes with. Not a public API.
#[doc(hidden)]
pub fn push<T: DeserializeOwned>(list: &mut Vec<T>, line: &[u8], number: usize) -> Result<usize> {
    list.push(crate::decode::from_jsonl_line(line, number)?);
    Ok(list.len() - 1)
}

/// The error of a line `BulkItem::__attach` has no place for. Not a public API.
#[doc(hidden)]
pub fn unplaced() -> Error {
    bulk_error("a nested line of the results has no place in the item")
}

/// A bulk query checked by `graphql!(bulk("…"))`, and its variables. Its
/// results are read as `I`, one top-level object at a time.
///
/// Shopify takes no variables for a bulk query: [`BulkQuery::text`] writes
/// them into the text.
pub struct BulkQuery<I> {
    operation: Operation<I>,
    template: &'static BulkTemplate,
}

impl<I> BulkQuery<I> {
    /// What `graphql!` builds.
    #[doc(hidden)]
    pub fn __checked(
        template: &'static BulkTemplate,
        query: &'static str,
        name: Option<&'static str>,
        checked_against: &'static str,
    ) -> Self {
        Self {
            operation: Operation::__checked(query, OperationKind::Query, name, checked_against),
            template,
        }
    }

    /// What the `Variables` of a `graphql!` module set a variable with.
    #[doc(hidden)]
    pub fn __variable<T: VariableType>(mut self, name: &str, value: T) -> Self {
        self.operation = self.operation.__variable(name, value);
        self
    }

    /// What `graphql!` sets a variable written as a Rust expression with.
    #[doc(hidden)]
    pub fn __converted_variable<T: VariableType, V: IntoVariable<T>>(
        mut self,
        name: &str,
        value: V,
    ) -> Self {
        self.operation = self.operation.variable(name, value);
        self
    }

    /// What `graphql!` sets a variable written as a literal to.
    #[doc(hidden)]
    pub fn __literal_variable(
        mut self,
        name: &str,
        value: std::result::Result<Box<RawValue>, Invalid>,
    ) -> Self {
        self.operation = self.operation.__literal_variable(name, value);
        self
    }

    /// The text of the query as written, with its variables.
    pub fn query(&self) -> &str {
        self.operation.query()
    }

    /// The Admin API version the query was checked against.
    pub fn checked_against(&self) -> Option<&'static str> {
        self.operation.checked_against()
    }

    /// The text Shopify runs: without variable definitions, each variable
    /// written in as a GraphQL literal (an enum value bare, an input object
    /// as `{field: value}`), and `__typename` selected on the nodes of the
    /// connections that share a parent, which tells their lines apart. A
    /// variable without a value is left out, as GraphQL leaves out an
    /// argument whose variable has none, or takes its default value.
    ///
    /// Fails with [`Error::InvalidValue`] for a value the variables refuse
    /// ([`Shopify::execute`] would), and for an enum value or a field name
    /// that is not a GraphQL name.
    pub fn text(&self) -> Result<String> {
        let values = self.operation.variable_values()?;
        let writer = Writer {
            template: self.template,
            values,
        };
        let mut text = String::new();
        for part in self.template.parts {
            match part {
                BulkPart::Text(part) => text.push_str(part),
                BulkPart::Arguments(arguments) => {
                    let written = writer.arguments(arguments)?;
                    if !written.is_empty() {
                        text.push('(');
                        text.push_str(&written.join(", "));
                        text.push(')');
                    }
                }
            }
        }
        Ok(text)
    }
}

impl<I> Clone for BulkQuery<I> {
    fn clone(&self) -> Self {
        Self {
            operation: self.operation.clone(),
            template: self.template,
        }
    }
}

impl<I> fmt::Debug for BulkQuery<I> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BulkQuery")
            .field("operation", &self.operation)
            .finish_non_exhaustive()
    }
}

/// Writes the variables of a bulk query into its text.
struct Writer<'a> {
    template: &'static BulkTemplate,
    values: &'a [(String, Box<RawValue>)],
}

impl Writer<'_> {
    /// `name: value` for each argument whose value is not left out.
    fn arguments(&self, arguments: &[BulkArgument]) -> Result<Vec<String>> {
        let mut written = Vec::new();
        for argument in arguments {
            if let Some(value) = self.value(&argument.value)? {
                written.push(format!("{}: {value}", argument.name));
            }
        }
        Ok(written)
    }

    /// The value as GraphQL text, `None` when its variable has no value.
    fn value(&self, value: &BulkValue) -> Result<Option<String>> {
        match value {
            BulkValue::Text(text) => Ok(Some((*text).to_string())),
            BulkValue::Variable(index) => {
                let variable = &self.template.variables[*index];
                match self.values.iter().find(|(name, _)| name == variable.name) {
                    Some((_, json)) => {
                        let mut text = String::new();
                        Json::new(json.get())
                            .write(&variable.ty, &mut text)
                            .map_err(|invalid| invalid.into_error(variable.name))?;
                        Ok(Some(text))
                    }
                    None => Ok(variable.default.map(str::to_string)),
                }
            }
            BulkValue::List(items) => {
                let mut written = Vec::new();
                for item in *items {
                    // An item whose variable has no value is `null`.
                    written.push(self.value(item)?.unwrap_or_else(|| "null".to_string()));
                }
                Ok(Some(format!("[{}]", written.join(", "))))
            }
            BulkValue::Object(fields) => {
                Ok(Some(format!("{{{}}}", self.arguments(fields)?.join(", "))))
            }
        }
    }
}

/// Reads the JSON of a value, which `serde_json` wrote, and writes it as a
/// GraphQL literal: strings and numbers as JSON writes them, which GraphQL
/// reads alike, enum values bare, object keys unquoted.
struct Json<'a> {
    text: &'a [u8],
    at: usize,
}

impl<'a> Json<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text: text.as_bytes(),
            at: 0,
        }
    }

    fn skip_whitespace(&mut self) {
        while self
            .text
            .get(self.at)
            .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'))
        {
            self.at += 1;
        }
    }

    /// The JSON text of the string that starts here, quotes included.
    fn string(&mut self) -> &'a str {
        let start = self.at;
        self.at += 1;
        while let Some(byte) = self.text.get(self.at) {
            match byte {
                b'\\' => self.at += 2,
                b'"' => {
                    self.at += 1;
                    break;
                }
                _ => self.at += 1,
            }
        }
        let end = self.at.min(self.text.len());
        // Cut at ASCII quotes of UTF-8 text: still UTF-8.
        std::str::from_utf8(&self.text[start..end]).unwrap_or("\"\"")
    }

    fn name(token: &str) -> std::result::Result<String, Invalid> {
        let name: String = serde_json::from_str(token)
            .map_err(|_| Invalid::new(InvalidValueReason::NotAGraphqlName))?;
        if is_graphql_name(&name) {
            Ok(name)
        } else {
            Err(Invalid::new(InvalidValueReason::NotAGraphqlName))
        }
    }

    /// An enum value: a GraphQL name but `true`, `false` and `null`, which
    /// would read as another kind of value.
    fn enum_value(token: &str) -> std::result::Result<String, Invalid> {
        let name = Self::name(token)?;
        if matches!(name.as_str(), "true" | "false" | "null") {
            Err(Invalid::new(InvalidValueReason::ReservedEnumValue))
        } else {
            Ok(name)
        }
    }

    fn write(&mut self, ty: &BulkType, out: &mut String) -> std::result::Result<(), Invalid> {
        self.skip_whitespace();
        match self.text.get(self.at) {
            Some(b'[') => {
                self.at += 1;
                let item = match ty {
                    BulkType::List(item) => item,
                    other => other,
                };
                out.push('[');
                let mut index = 0;
                loop {
                    self.skip_whitespace();
                    match self.text.get(self.at) {
                        Some(b']') | None => {
                            self.at += 1;
                            break;
                        }
                        Some(b',') => {
                            self.at += 1;
                            continue;
                        }
                        _ => {}
                    }
                    if index > 0 {
                        out.push_str(", ");
                    }
                    self.write(item, out)
                        .map_err(|invalid| invalid.index(index))?;
                    index += 1;
                }
                out.push(']');
            }
            Some(b'{') => {
                self.at += 1;
                let fields = match element(ty) {
                    BulkType::Input(fields) => *fields,
                    _ => &[],
                };
                out.push('{');
                let mut first = true;
                loop {
                    self.skip_whitespace();
                    match self.text.get(self.at) {
                        Some(b'}') | None => {
                            self.at += 1;
                            break;
                        }
                        Some(b',') => {
                            self.at += 1;
                            continue;
                        }
                        _ => {}
                    }
                    let key = self.string();
                    let key = Self::name(key)?;
                    self.skip_whitespace();
                    // The `:` after the key.
                    self.at += 1;
                    if !first {
                        out.push_str(", ");
                    }
                    first = false;
                    out.push_str(&key);
                    out.push_str(": ");
                    match fields.iter().find(|(name, _)| *name == key) {
                        Some((name, field)) => self
                            .write(field, out)
                            .map_err(|invalid| invalid.field(name))?,
                        None => self.write(&BulkType::Plain, out)?,
                    }
                }
                out.push('}');
            }
            Some(b'"') => {
                let token = self.string();
                if matches!(element(ty), BulkType::Enum) {
                    out.push_str(&Self::enum_value(token)?);
                } else {
                    out.push_str(token);
                }
            }
            _ => {
                // `true`, `false`, `null` or a number, which read alike.
                let start = self.at;
                while self.text.get(self.at).is_some_and(|byte| {
                    !matches!(byte, b',' | b']' | b'}' | b' ' | b'\t' | b'\n' | b'\r')
                }) {
                    self.at += 1;
                }
                out.push_str(std::str::from_utf8(&self.text[start..self.at]).unwrap_or("null"));
            }
        }
        Ok(())
    }
}

/// The type of a value that is not a list where `ty` stands: GraphQL reads
/// a single value where a list is expected as its one item.
fn element(ty: &BulkType) -> &BulkType {
    match ty {
        BulkType::List(item) => element(item),
        other => other,
    }
}

/// One line of a bulk mutation checked by `graphql!(bulk("mutation …"))`:
/// the variables of one run of the mutation.
/// [`Shopify::bulk_mutation`] runs an iterator of them.
pub struct BulkMutation<D> {
    operation: Operation<D>,
}

impl<D> BulkMutation<D> {
    /// What `graphql!` builds.
    #[doc(hidden)]
    pub fn __checked(
        query: &'static str,
        name: Option<&'static str>,
        checked_against: &'static str,
    ) -> Self {
        Self {
            operation: Operation::__checked(query, OperationKind::Mutation, name, checked_against),
        }
    }

    /// What the `Variables` of a `graphql!` module set a variable with.
    #[doc(hidden)]
    pub fn __variable<T: VariableType>(mut self, name: &str, value: T) -> Self {
        self.operation = self.operation.__variable(name, value);
        self
    }

    /// What `graphql!` sets a variable written as a Rust expression with.
    #[doc(hidden)]
    pub fn __converted_variable<T: VariableType, V: IntoVariable<T>>(
        mut self,
        name: &str,
        value: V,
    ) -> Self {
        self.operation = self.operation.variable(name, value);
        self
    }

    /// What `graphql!` sets a variable written as a literal to.
    #[doc(hidden)]
    pub fn __literal_variable(
        mut self,
        name: &str,
        value: std::result::Result<Box<RawValue>, Invalid>,
    ) -> Self {
        self.operation = self.operation.__literal_variable(name, value);
        self
    }

    /// The text of the mutation.
    pub fn query(&self) -> &str {
        self.operation.query()
    }

    /// The Admin API version the mutation was checked against.
    pub fn checked_against(&self) -> Option<&'static str> {
        self.operation.checked_against()
    }

    /// The line of the JSONL file of variables: the variables as one JSON
    /// object. Fails as [`Shopify::execute`] would for a value refused.
    pub fn line(&self) -> Result<String> {
        let values = self.operation.variable_values()?;
        let mut line = String::from("{");
        for (index, (name, value)) in values.iter().enumerate() {
            if index > 0 {
                line.push(',');
            }
            line.push_str(
                &serde_json::to_string(name).map_err(|error| Error::variables(None, &error))?,
            );
            line.push(':');
            line.push_str(value.get());
        }
        line.push('}');
        Ok(line)
    }
}

impl<D> Clone for BulkMutation<D> {
    fn clone(&self) -> Self {
        Self {
            operation: self.operation.clone(),
        }
    }
}

impl<D> fmt::Debug for BulkMutation<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BulkMutation")
            .field("operation", &self.operation)
            .finish()
    }
}

/// A line of the results of a bulk mutation: what one run of the mutation
/// returned. Shopify runs the lines in no set order: `index` tells which
/// line of variables it ran with.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[non_exhaustive]
pub struct BulkMutationResult<D> {
    /// The `data` of the mutation; its `userErrors` are in it.
    #[serde(default = "none")]
    pub data: Option<D>,
    /// The errors of the run: a variable Shopify refused, a missing scope.
    #[serde(default)]
    pub errors: Vec<GraphqlError>,
    /// The line of variables it ran with, from 0: Shopify's `__lineNumber`.
    /// Shopify's documentation does not show it on every line of errors.
    #[serde(rename = "__lineNumber", default)]
    pub index: Option<u64>,
}

fn none<D>() -> Option<D> {
    None
}

/// The lines of a bulk mutation, checked and written as its JSONL file.
struct Lines {
    mutation: String,
    checked_against: Option<&'static str>,
    jsonl: Vec<u8>,
}

impl Lines {
    /// `None` when there is no line.
    fn of<D, L: Into<BulkMutation<D>>>(lines: impl IntoIterator<Item = L>) -> Result<Option<Self>> {
        let mut lines = lines.into_iter().map(Into::into);
        let Some(first) = lines.next() else {
            return Ok(None);
        };
        let mut jsonl = first.line()?.into_bytes();
        jsonl.push(b'\n');
        for (index, line) in lines.enumerate() {
            if line.query() != first.query() {
                return Err(bulk_error(format!(
                    "line {} of the bulk mutation was built from another mutation than line 1: a bulk operation runs one mutation",
                    index + 2
                )));
            }
            jsonl.extend_from_slice(line.line()?.as_bytes());
            jsonl.push(b'\n');
        }
        Ok(Some(Self {
            mutation: first.query().to_string(),
            checked_against: first.checked_against(),
            jsonl,
        }))
    }
}

impl Shopify {
    /// Runs a bulk query declared with `graphql!(bulk(…))` to its end, then
    /// reads its results one top-level object at a time, with the objects
    /// of its nested connections in their lists, as they download (see
    /// [`BulkItems`]). Fails when the operation does not complete.
    ///
    /// It runs with `groupObjects`, which [`BulkItems`] needs: Shopify is
    /// slower with it, and more likely to time out.
    ///
    /// ```no_run
    /// use shopify_api::bulk::BulkWaitOptions;
    ///
    /// shopify_api::graphql! {
    ///     mod all_products = bulk("query($filter: String) {
    ///         products(query: $filter) { edges { node {
    ///             id title
    ///             variants { edges { node { sku } } }
    ///         } } }
    ///     }");
    /// }
    ///
    /// # async fn run(shopify: shopify_api::Shopify) -> shopify_api::Result<()> {
    /// let mut products = shopify
    ///     .bulk_query(
    ///         all_products::Variables { filter: Some("status:active".into()) },
    ///         BulkWaitOptions::default(),
    ///     )
    ///     .await?;
    /// while let Some(product) = products.next_item().await {
    ///     let product = product?;
    ///     println!("{} has {} variants", product.title, product.variants.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_query<I: BulkItem>(
        &self,
        query: impl Into<BulkQuery<I>>,
        options: BulkWaitOptions,
    ) -> Result<BulkItems<I>> {
        let operation = self.start_bulk_query(query).await?;
        match self.completed_url(&operation.id, options).await? {
            Some(url) => Ok(self.download_bulk_items(&url).await?.grouped()),
            None => Ok(BulkItems::new(None, "").grouped()),
        }
    }

    /// Starts a bulk query declared with `graphql!(bulk(…))`, with
    /// `groupObjects`, and returns the operation: wait for it with
    /// [`Shopify::wait_for_bulk_operation`], then read its results with
    /// [`Shopify::download_bulk_items`].
    ///
    /// It sends [`BulkQuery::text`] to the Admin API version the query was
    /// checked against, as [`Shopify::execute`] would.
    pub async fn start_bulk_query<I>(
        &self,
        query: impl Into<BulkQuery<I>>,
    ) -> Result<BulkOperation> {
        let query = query.into();
        let text = query.text()?;
        let operation = Operation::from(run_query::Variables {
            query: text,
            group_objects: true,
        })
        .checked_as(query.checked_against());
        let data = self.execute(operation).await?;
        let payload = data
            .bulk_operation_run_query
            .ok_or_else(|| bulk_error("bulkOperationRunQuery returned nothing"))?;
        started(
            "bulkOperationRunQuery",
            payload.bulk_operation,
            &payload.user_errors,
        )
    }

    /// Downloads the results of a bulk query started with
    /// [`Shopify::start_bulk_query`], its `url` or its `partial_data_url`,
    /// and reads them as [`Shopify::bulk_query`] does. The item type names
    /// the query: `all_products::Item`.
    ///
    /// ```no_run
    /// use shopify_api::bulk::{BulkOperationStatus, BulkWaitOptions};
    ///
    /// shopify_api::graphql! {
    ///     mod all_products = bulk("{ products { edges { node { id title } } } }");
    /// }
    ///
    /// # async fn run(shopify: shopify_api::Shopify) -> shopify_api::Result<()> {
    /// let started = shopify.start_bulk_query(all_products::operation()).await?;
    /// let operation = shopify
    ///     .wait_for_bulk_operation(&started.id, BulkWaitOptions::default())
    ///     .await?;
    /// if let (BulkOperationStatus::Completed, Some(url)) = (&operation.status, &operation.url) {
    ///     let products: Vec<all_products::Item> =
    ///         shopify.download_bulk_items(url).await?.collect().await?;
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn download_bulk_items<I: BulkItem>(&self, url: &str) -> Result<BulkItems<I>> {
        let body = self.open_bulk_results(url).await?;
        Ok(BulkItems::new(Some(body), url))
    }

    /// Runs a bulk mutation declared with `graphql!(bulk("mutation …"))`
    /// over `lines`, the variables of each run, to its end; then reads the
    /// result of each run as it downloads. Fails when the operation does
    /// not complete.
    ///
    /// Every line is checked and written before anything is sent: a value
    /// refused fails as [`Shopify::execute`] would, and a line of another
    /// mutation with [`Error::Bulk`]. Without lines, nothing runs and no
    /// result is read.
    ///
    /// ```no_run
    /// use shopify_api::bulk::BulkWaitOptions;
    ///
    /// shopify_api::graphql! {
    ///     mod add_tags = bulk("mutation($id: ID!, $tags: [String!]!) {
    ///         tagsAdd(id: $id, tags: $tags) { userErrors { field message } }
    ///     }");
    /// }
    ///
    /// # async fn run(shopify: shopify_api::Shopify) -> shopify_api::Result<()> {
    /// let ids = ["gid://shopify/Product/1", "gid://shopify/Product/2"];
    /// let lines = ids.iter().map(|id| add_tags::Variables {
    ///     id: id.to_string(),
    ///     tags: vec!["sale".to_string()],
    /// });
    /// let mut results = shopify.bulk_mutation(lines, BulkWaitOptions::default()).await?;
    /// while let Some(result) = results.next_row().await {
    ///     let result = result?;
    ///     let errors = result.data.and_then(|data| data.tags_add).map(|payload| payload.user_errors);
    ///     println!("line {:?}: {errors:?}", result.index);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn bulk_mutation<D, L>(
        &self,
        lines: impl IntoIterator<Item = L>,
        options: BulkWaitOptions,
    ) -> Result<BulkRows<BulkMutationResult<D>>>
    where
        D: DeserializeOwned,
        L: Into<BulkMutation<D>>,
    {
        let Some(lines) = Lines::of(lines)? else {
            return Ok(BulkRows::new(None, ""));
        };
        let operation = self.start_lines(lines).await?;
        match self.completed_url(&operation.id, options).await? {
            Some(url) => self.download_bulk_results_stream(&url).await,
            None => Ok(BulkRows::new(None, "")),
        }
    }

    /// Starts a bulk mutation declared with `graphql!(bulk("mutation …"))`
    /// over `lines`, and returns the operation. Its results read as
    /// [`BulkMutationResult`]s with [`Shopify::download_bulk_results_stream`].
    ///
    /// Every line is checked and written before anything is sent, as
    /// [`Shopify::bulk_mutation`] does. Fails with [`Error::Bulk`] without lines.
    pub async fn start_bulk_mutation<D, L>(
        &self,
        lines: impl IntoIterator<Item = L>,
    ) -> Result<BulkOperation>
    where
        L: Into<BulkMutation<D>>,
    {
        let lines = Lines::of(lines)?
            .ok_or_else(|| bulk_error("a bulk mutation needs one line of variables at least"))?;
        self.start_lines(lines).await
    }

    async fn start_lines(&self, lines: Lines) -> Result<BulkOperation> {
        let jsonl = lines.jsonl;
        let path = self
            .stage_jsonl(move |body| {
                body.extend_from_slice(&jsonl);
                Ok(())
            })
            .await?;
        let operation = Operation::from(run_mutation::Variables {
            mutation: lines.mutation,
            staged_upload_path: path,
            client_identifier: None,
        })
        .checked_as(lines.checked_against);
        let data = self.execute(operation).await?;
        let payload = data
            .bulk_operation_run_mutation
            .ok_or_else(|| bulk_error("bulkOperationRunMutation returned nothing"))?;
        started(
            "bulkOperationRunMutation",
            payload.bulk_operation,
            &payload.user_errors,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(json: &str, ty: &BulkType) -> std::result::Result<String, String> {
        let mut out = String::new();
        Json::new(json)
            .write(ty, &mut out)
            .map(|()| out)
            .map_err(|invalid| invalid.into_error("v").to_string())
    }

    static COLORS: BulkType = BulkType::List(&BulkType::Enum);
    static INPUT_FIELDS: [(&str, BulkType); 2] = [
        ("status", BulkType::Enum),
        ("nested", BulkType::Input(&INPUT_FIELDS)),
    ];
    static INPUT: BulkType = BulkType::Input(&INPUT_FIELDS);

    #[test]
    fn strings_keep_the_escapes_of_json() {
        for (value, expected) in [
            ("plain", r#""plain""#),
            ("a \"quoted\" word", r#""a \"quoted\" word""#),
            ("back\\slash", r#""back\\slash""#),
            ("two\nlines\r\tand a tab", r#""two\nlines\r\tand a tab""#),
            ("café ☕ 𝄞", "\"café ☕ 𝄞\""),
            ("\u{1}\u{1f}", r#""\u0001\u001f""#),
            ("$filter and ${x}", r#""$filter and ${x}""#),
            ("\"\"\" block", r#""\"\"\" block""#),
        ] {
            let json = serde_json::to_string(value).unwrap();
            assert_eq!(
                written(&json, &BulkType::Plain).unwrap(),
                expected,
                "{value:?}"
            );
            // GraphQL reads it back as the same string.
            let query = format!("{{ f(a: {expected}) }}");
            let document = graphql_parser::parse_query::<String>(&query).unwrap();
            let graphql_parser::query::Definition::Operation(
                graphql_parser::query::OperationDefinition::SelectionSet(set),
            ) = &document.definitions[0]
            else {
                panic!("a selection set");
            };
            let graphql_parser::query::Selection::Field(field) = &set.items[0] else {
                panic!("a field");
            };
            assert_eq!(
                field.arguments[0].1,
                graphql_parser::query::Value::String(value.to_string()),
                "{value:?}"
            );
        }
    }

    #[test]
    fn scalars_are_written_as_json_writes_them() {
        assert_eq!(written("12", &BulkType::Plain).unwrap(), "12");
        assert_eq!(written("-1.5e300", &BulkType::Plain).unwrap(), "-1.5e300");
        assert_eq!(written("true", &BulkType::Plain).unwrap(), "true");
        assert_eq!(written("null", &BulkType::Plain).unwrap(), "null");
        assert_eq!(written("null", &BulkType::Enum).unwrap(), "null");
        assert_eq!(
            written("[[1,2],[],[null]]", &BulkType::Plain).unwrap(),
            "[[1, 2], [], [null]]"
        );
    }

    #[test]
    fn enum_values_are_bare_wherever_they_stand() {
        assert_eq!(written(r#""ACTIVE""#, &BulkType::Enum).unwrap(), "ACTIVE");
        assert_eq!(
            written(r#"["RED","BLUE"]"#, &COLORS).unwrap(),
            "[RED, BLUE]"
        );
        // One value where a list is expected.
        assert_eq!(written(r#""RED""#, &COLORS).unwrap(), "RED");
        assert_eq!(
            written(
                r#"{"status":"DRAFT","title":"A \"b\"","nested":{"status":"ACTIVE","tags":["X"]}}"#,
                &INPUT
            )
            .unwrap(),
            r#"{status: DRAFT, title: "A \"b\"", nested: {status: ACTIVE, tags: ["X"]}}"#
        );
        assert_eq!(
            written(
                r#"[{"status":"DRAFT"},{"status":null}]"#,
                &BulkType::List(&INPUT)
            )
            .unwrap(),
            "[{status: DRAFT}, {status: null}]"
        );
        // A string is not an enum value where none is expected.
        assert_eq!(
            written(r#""ACTIVE""#, &BulkType::Plain).unwrap(),
            r#""ACTIVE""#
        );
    }

    #[test]
    fn what_is_not_a_graphql_name_is_refused() {
        for (json, ty, path) in [
            (r#""ACTIVE) { x }""#, &BulkType::Enum, "$v"),
            (r#""""#, &BulkType::Enum, "$v"),
            (r#"["RED","1BLUE"]"#, &COLORS, "$v[1]"),
            (
                r#"{"nested":{"status":"no way"}}"#,
                &INPUT,
                "$v.nested.status",
            ),
            (r#"{"a b":1}"#, &BulkType::Plain, "$v"),
        ] {
            assert_eq!(
                written(json, ty).unwrap_err(),
                format!(
                    "invalid value at `{path}`: not a GraphQL name, as an enum value or a field written into a bulk query must be"
                ),
                "{json}"
            );
        }
    }
}
