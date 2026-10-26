//! Bulk operations: Shopify's rules for the query of
//! `bulkOperationRunQuery` and the mutation of `bulkOperationRunMutation`,
//! checked on top of [`analyze`](crate::analyze()), and what a typed bulk
//! query needs at run time: the text it sends, and where each line of its
//! results goes.
//!
//! The rules follow Shopify's guide for API 2026-10
//! (<https://shopify.dev/docs/api/usage/bulk-operations/queries>). Where it
//! is silent, what it does not show is refused:
//!
//! - one top-level field, a connection, not `node` or `nodes`;
//! - at most five connections, nested at most two levels below it;
//! - each connection selected as `edges { node { … } }` and nothing else:
//!   no `nodes`, `cursor`, `pageInfo`, and no `first`, `last`, `after` or
//!   `before`, which Shopify ignores;
//! - the nodes of a connection implement `Node`;
//! - a nested connection written in the `node { … }` of its parent, not in
//!   a fragment or under another field;
//! - an object with nested connections selects `id`, without an alias,
//!   and no other field under the key `id`;
//! - two connections of one object that may return the same type are
//!   refused: their lines could not be told apart.
//!
//! A bulk query takes no variables. Its variables are written into its text
//! as GraphQL literals at run time: the text is kept as [`Part`]s, with the
//! argument lists that hold a variable apart.

mod strings;

use std::collections::{BTreeMap, BTreeSet};

use graphql_parser::Pos;
use graphql_parser::query as ast;
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};

use crate::analyze::{Error, FieldBase, Operation, OperationKind, SelectedField, Selection};
use crate::codegen::{field_idents, item_types};
use crate::schema::{Field, Schema, TypeDef, TypeRef};

type Document<'a> = ast::Document<'a, String>;
type SelectionSet<'a> = ast::SelectionSet<'a, String>;
type Value<'a> = ast::Value<'a, String>;
type Fragments<'a> = BTreeMap<&'a str, &'a ast::FragmentDefinition<'a, String>>;

/// The most connections a bulk query may have, its top-level one included.
pub const MAX_CONNECTIONS: usize = 5;
/// How many levels of connections a bulk query may nest below its top-level one.
pub const MAX_DEPTH: usize = 2;

/// The name of the type generated for each top-level object of a bulk query.
pub const ITEM_TYPE: &str = "Item";

/// The alias of the `__typename` written on the nodes of the connections
/// that share a parent, which tells their lines apart. The lines are routed
/// by it alone: a `__typename` the query selects is left as written.
pub const TYPENAME_KEY: &str = "_shopifyApiTypename";

/// Arguments Shopify ignores in a bulk query: it reads every object.
const PAGINATION: [&str; 4] = ["first", "last", "after", "before"];

fn error_at<T>(position: Pos, message: impl Into<String>) -> Result<T, Error> {
    Err(Error {
        message: message.into(),
        position: Some((position.line, position.column)),
    })
}

fn error<T>(message: impl Into<String>) -> Result<T, Error> {
    Err(Error {
        message: message.into(),
        position: None,
    })
}

/// A checked bulk query.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct BulkQuery {
    /// The selection on the nodes of the top-level connection, each nested
    /// connection made a list of its nodes ([`SelectedField::connection`]).
    pub item: Selection,
    /// The nested connections of the top-level nodes, in the order of their
    /// fields in [`BulkQuery::item`].
    pub connections: Vec<Connection>,
    /// The text sent, variables apart.
    pub parts: Vec<Part>,
    /// The default value of each variable that has one, as GraphQL text.
    pub defaults: BTreeMap<String, String>,
}

/// A nested connection, and where its lines go.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Connection {
    /// Its response key.
    pub key: String,
    /// The object types its nodes may be, which their `__typename` names.
    pub types: Vec<String>,
    /// The connections of its nodes.
    pub connections: Vec<Connection>,
}

/// A piece of the text of a bulk query.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Part {
    Text(String),
    /// `(name: value, …)` where a value holds a variable. An argument whose
    /// variable has no value and no default is left out, and the
    /// parentheses with the last one.
    Arguments(Vec<Argument>),
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Argument {
    pub name: String,
    pub value: ValuePart,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ValuePart {
    /// A constant, as GraphQL text.
    Text(String),
    Variable(String),
    List(Vec<ValuePart>),
    /// An input object: a field whose variable has no value is left out.
    Object(Vec<Argument>),
}

/// The object types a value of `name` may be, sorted.
fn concrete_types(schema: &Schema, name: &str) -> Vec<String> {
    let mut types: Vec<String> = schema
        .possible_types(name)
        .into_iter()
        .map(str::to_string)
        .collect();
    types.sort();
    types.dedup();
    types
}

fn fields_of<'s>(schema: &'s Schema, name: &str) -> Option<&'s BTreeMap<String, Field>> {
    match schema.get(name) {
        Some(TypeDef::Object { fields, .. } | TypeDef::Interface { fields, .. }) => Some(fields),
        _ => None,
    }
}

/// The node type of a connection type: an object with `edges { node }`
/// and `pageInfo`, as Shopify's connections are.
pub fn connection_node<'s>(schema: &'s Schema, name: &str) -> Option<&'s str> {
    let Some(TypeDef::Object { fields, .. }) = schema.get(name) else {
        return None;
    };
    if !fields.contains_key("pageInfo") {
        return None;
    }
    let edge = fields.get("edges")?.ty.name();
    let Some(TypeDef::Object { fields: edge, .. }) = schema.get(edge) else {
        return None;
    };
    Some(edge.get("node")?.ty.name())
}

/// Every value of `name` implements `Node`.
fn implements_node(schema: &Schema, name: &str) -> bool {
    let types = schema.possible_types(name);
    !types.is_empty()
        && types.iter().all(|ty| {
            matches!(schema.get(ty), Some(TypeDef::Object { interfaces, .. }) if interfaces.iter().any(|interface| interface == "Node"))
        })
}

fn parse(source: &str) -> Result<Document<'_>, Error> {
    // `analyze` read it first: a failure here is not expected.
    graphql_parser::parse_query(source).or_else(|_| error("Invalid GraphQL"))
}

/// The operation and the fragments of a document `analyze` accepted.
fn parts_of<'a>(
    document: &'a Document<'a>,
) -> (&'a ast::OperationDefinition<'a, String>, Fragments<'a>) {
    let mut operation = None;
    let mut fragments = BTreeMap::new();
    for definition in &document.definitions {
        match definition {
            ast::Definition::Operation(found) => operation = Some(found),
            ast::Definition::Fragment(fragment) => {
                fragments.insert(fragment.name.as_str(), fragment);
            }
        }
    }
    (
        operation.expect("`analyze` accepted one operation"),
        fragments,
    )
}

fn selection_set<'a>(operation: &'a ast::OperationDefinition<'a, String>) -> &'a SelectionSet<'a> {
    match operation {
        ast::OperationDefinition::SelectionSet(set) => set,
        ast::OperationDefinition::Query(query) => &query.selection_set,
        ast::OperationDefinition::Mutation(mutation) => &mutation.selection_set,
        ast::OperationDefinition::Subscription(subscription) => &subscription.selection_set,
    }
}

fn set_position(set: &SelectionSet<'_>) -> Pos {
    set.span.0
}

fn item_position(item: &ast::Selection<'_, String>) -> Pos {
    match item {
        ast::Selection::Field(field) => field.position,
        ast::Selection::InlineFragment(fragment) => fragment.position,
        ast::Selection::FragmentSpread(spread) => spread.position,
    }
}

/// The only field of a top-level selection, or an error that says why.
fn top_level_field<'a>(
    set: &'a SelectionSet<'a>,
    what: &str,
) -> Result<&'a ast::Field<'a, String>, Error> {
    match set.items.as_slice() {
        [ast::Selection::Field(field)] => Ok(field),
        [_, second, ..] => error_at(
            item_position(second),
            format!("A bulk {what} selects one top-level field: Shopify runs one per operation"),
        ),
        [other] => error_at(
            item_position(other),
            format!("A bulk {what} selects one top-level field, not a fragment"),
        ),
        [] => error_at(
            set_position(set),
            format!("A bulk {what} selects one top-level field"),
        ),
    }
}

/// Checks the bulk rules of a query `analyze` accepted, and describes
/// what runs it.
pub fn analyze_bulk_query(
    schema: &Schema,
    source: &str,
    operation: &Operation,
) -> Result<BulkQuery, Error> {
    if operation.kind != OperationKind::Query {
        return error("A bulk query is a query");
    }
    strings::check(source)?;
    let document = parse(source)?;
    let (definition, fragments) = parts_of(&document);
    let mut checker = Checker {
        schema,
        fragments: &fragments,
        count: 0,
        typename: BTreeSet::new(),
    };
    checker.variables(definition)?;

    let top = top_level_field(selection_set(definition), "query")?;
    if top.name == "node" || top.name == "nodes" {
        return error_at(
            top.position,
            format!(
                "`{}` cannot be the top-level field of a bulk query: select a connection, such as `products`",
                top.name
            ),
        );
    }
    let ty = fields_of(schema, &schema.query)
        .and_then(|fields| fields.get(&top.name))
        .map(|field| field.ty.clone())
        .ok_or_else(|| Error {
            message: format!("The query root has no field `{}`", top.name),
            position: Some((top.position.line, top.position.column)),
        })?;
    if connection_node(schema, ty.name()).is_none() || matches!(ty.nullable(), TypeRef::List(_)) {
        return error_at(
            top.position,
            format!(
                "The top-level field of a bulk query is a connection: `{}` is `{ty}`. Run it as an ordinary query, or select a connection such as `products`",
                top.name
            ),
        );
    }
    let top_connection = checker.connection(top, &schema.query, 0)?;

    let data_field = operation
        .data
        .fields
        .first()
        .expect("the selection is not empty");
    let item = item_selection(schema, connection_nodes(data_field));

    let mut printer = Printer {
        typename: &checker.typename,
        parts: Vec::new(),
        text: String::new(),
    };
    printer.operation(definition);
    for fragment in fragments.values() {
        printer.text.push(' ');
        printer.fragment(fragment);
    }
    let parts = printer.finish();

    let mut defaults = BTreeMap::new();
    if let ast::OperationDefinition::Query(query) = definition {
        for variable in &query.variable_definitions {
            if let Some(default) = &variable.default_value {
                defaults.insert(variable.name.clone(), constant_text(default));
            }
        }
    }

    Ok(BulkQuery {
        item,
        connections: top_connection.connections,
        parts,
        defaults,
    })
}

/// Checks the bulk rules of a mutation `analyze` accepted: one mutation
/// field, not a bulk operation itself, with at most one connection.
pub fn check_bulk_mutation(
    schema: &Schema,
    source: &str,
    operation: &Operation,
) -> Result<(), Error> {
    if operation.kind != OperationKind::Mutation {
        return error("A bulk mutation is a mutation");
    }
    let document = parse(source)?;
    let (definition, fragments) = parts_of(&document);
    let top = top_level_field(selection_set(definition), "mutation")?;
    if matches!(
        top.name.as_str(),
        "bulkOperationRunMutation" | "bulkOperationRunQuery"
    ) {
        return error_at(
            top.position,
            format!("Shopify does not run `{}` in a bulk mutation", top.name),
        );
    }
    let root = schema.mutation.as_deref().unwrap_or_default();
    let ty = fields_of(schema, root)
        .and_then(|fields| fields.get(&top.name))
        .map(|field| field.ty.name().to_string())
        .unwrap_or_default();
    let mut found = Vec::new();
    find_connections(schema, &fragments, &top.selection_set, &ty, &mut found, 0);
    if let [first, second, ..] = found.as_slice() {
        return error_at(
            second.1,
            format!(
                "The mutation of a bulk operation selects one connection at most (`{}`), Shopify's limit: `{}` is a second one",
                first.0, second.0
            ),
        );
    }
    Ok(())
}

/// The connection fields selected in `set`, at any depth, with where they are.
fn find_connections<'a>(
    schema: &Schema,
    fragments: &Fragments<'a>,
    set: &'a SelectionSet<'a>,
    parent: &str,
    found: &mut Vec<(String, Pos)>,
    depth: usize,
) {
    // Fragments do not nest without end: `analyze` refused that.
    if depth > 64 {
        return;
    }
    for item in &set.items {
        match item {
            ast::Selection::Field(field) => {
                let Some(ty) = fields_of(schema, parent)
                    .and_then(|fields| fields.get(&field.name))
                    .map(|field| field.ty.name().to_string())
                else {
                    continue;
                };
                if connection_node(schema, &ty).is_some() {
                    found.push((field.name.clone(), field.position));
                }
                find_connections(
                    schema,
                    fragments,
                    &field.selection_set,
                    &ty,
                    found,
                    depth + 1,
                );
            }
            ast::Selection::InlineFragment(fragment) => {
                let target = match &fragment.type_condition {
                    Some(ast::TypeCondition::On(name)) => name.as_str(),
                    None => parent,
                };
                find_connections(
                    schema,
                    fragments,
                    &fragment.selection_set,
                    target,
                    found,
                    depth + 1,
                );
            }
            ast::Selection::FragmentSpread(spread) => {
                if let Some(fragment) = fragments.get(spread.fragment_name.as_str()) {
                    let ast::TypeCondition::On(target) = &fragment.type_condition;
                    find_connections(
                        schema,
                        fragments,
                        &fragment.selection_set,
                        target,
                        found,
                        depth + 1,
                    );
                }
            }
        }
    }
}

struct Checker<'a, 's> {
    schema: &'s Schema,
    fragments: &'a Fragments<'a>,
    /// The connections seen so far.
    count: usize,
    /// The `node` fields whose selection gains `__typename`, aliased [`TYPENAME_KEY`].
    typename: BTreeSet<(usize, usize)>,
}

impl<'a> Checker<'a, '_> {
    /// The variables are written into the text: a `JSON` value has no
    /// GraphQL literal this crate can write for sure.
    fn variables(&self, definition: &ast::OperationDefinition<'_, String>) -> Result<(), Error> {
        let ast::OperationDefinition::Query(query) = definition else {
            return Ok(());
        };
        for variable in &query.variable_definitions {
            let mut seen = BTreeSet::new();
            let name = base_name(&variable.var_type);
            if reaches_json(self.schema, name, &mut seen) {
                return error_at(
                    variable.position,
                    format!(
                        "Variable `${}` may hold a `JSON` value, which a bulk query cannot take: Shopify takes no variables for a bulk query, and this crate writes them into its text as GraphQL literals",
                        variable.name
                    ),
                );
            }
        }
        Ok(())
    }

    /// Checks a connection field of `parent` and what it selects, `depth`
    /// levels below the top-level one.
    fn connection(
        &mut self,
        field: &'a ast::Field<'a, String>,
        parent: &str,
        depth: usize,
    ) -> Result<Connection, Error> {
        self.count += 1;
        if self.count > MAX_CONNECTIONS {
            return error_at(
                field.position,
                format!(
                    "A bulk query has {MAX_CONNECTIONS} connections at most: `{}` is connection number {}",
                    field.name, self.count
                ),
            );
        }
        if depth > MAX_DEPTH {
            return error_at(
                field.position,
                format!(
                    "Connections nest {MAX_DEPTH} levels deep at most below the top-level one in a bulk query: `{}` is level {depth}",
                    field.name
                ),
            );
        }
        if let Some(directive) = field.directives.first() {
            return error_at(
                directive.position,
                format!(
                    "A connection of a bulk query cannot have directives such as `@{}`: Shopify writes its nodes on lines of their own",
                    directive.name
                ),
            );
        }
        if let Some((argument, _)) = field
            .arguments
            .iter()
            .find(|(name, _)| PAGINATION.contains(&name.as_str()))
        {
            return error_at(
                field.position,
                format!(
                    "Shopify ignores `{argument}` in a bulk query and returns every object of `{}`: remove it",
                    field.name
                ),
            );
        }
        let shape = format!(
            "A connection of a bulk query selects `edges {{ node {{ … }} }}` and nothing else, as Shopify documents it (`{}`)",
            field.name
        );
        let edges = only_field(&field.selection_set, "edges", &shape)?;
        let node = only_field(&edges.selection_set, "node", &shape)?;

        let ty = self.field_type(parent, field).unwrap_or_default();
        let node_type = connection_node(self.schema, &ty)
            .unwrap_or_default()
            .to_string();
        if !implements_node(self.schema, &node_type) {
            return error_at(
                field.position,
                format!(
                    "Shopify bulk queries read connections of `Node` objects: the nodes of `{}`, `{node_type}`, do not all implement `Node`",
                    field.name
                ),
            );
        }

        let children = self.node_selection(&node.selection_set, &node_type, depth)?;
        if let Some(child) = children.first() {
            if let Some(other) = self.aliased_id(&node.selection_set, 0) {
                return error_at(
                    other.position,
                    format!(
                        "`id: {}` is refused on the nodes of `{}`, which have the nested connection `{}`: each nested line is placed under its parent by the `id` of that parent. Give `{}` another alias",
                        other.name, field.name, child.name, other.name
                    ),
                );
            }
            let has_id = node.selection_set.items.iter().any(|item| {
                matches!(item, ast::Selection::Field(id) if id.name == "id" && id.alias.is_none() && id.directives.is_empty())
            });
            if !has_id {
                return error_at(
                    node.position,
                    format!(
                        "The nodes of `{}` have the nested connection `{}`: select their `id`, without an alias or a directive, in this `node {{ … }}`. Shopify refuses the query otherwise, and places each nested line under its parent by that `id`",
                        field.name, child.name
                    ),
                );
            }
        }
        if children.len() > 1 {
            for (index, later) in children.iter().enumerate() {
                for earlier in &children[..index] {
                    if let Some(shared) = earlier
                        .connection
                        .types
                        .iter()
                        .find(|ty| later.connection.types.contains(ty))
                    {
                        return error_at(
                            later.position,
                            format!(
                                "`{}` and `{}`, connections of each node of `{}`, may both return `{shared}`: their lines in the results could not be told apart. Query one of them in another bulk query",
                                earlier.connection.key, later.connection.key, field.name
                            ),
                        );
                    }
                }
            }
            for child in &children {
                self.typename.insert((child.node.line, child.node.column));
            }
        }
        Ok(Connection {
            key: field.alias.as_ref().unwrap_or(&field.name).clone(),
            types: concrete_types(self.schema, &node_type),
            connections: children.into_iter().map(|child| child.connection).collect(),
        })
    }

    /// The type name of `field` on `parent`.
    fn field_type(&self, parent: &str, field: &ast::Field<'_, String>) -> Option<String> {
        fields_of(self.schema, parent)
            .and_then(|fields| fields.get(&field.name))
            .map(|definition| definition.ty.name().to_string())
    }

    fn field_definition(&self, parent: &str, field: &ast::Field<'_, String>) -> Option<&Field> {
        fields_of(self.schema, parent).and_then(|fields| fields.get(&field.name))
    }

    /// A field other than `id` aliased `id` in `set` or its fragments.
    fn aliased_id(
        &self,
        set: &'a SelectionSet<'a>,
        depth: usize,
    ) -> Option<&'a ast::Field<'a, String>> {
        // Fragments do not nest without end: `analyze` refused that.
        if depth > 64 {
            return None;
        }
        set.items.iter().find_map(|item| match item {
            ast::Selection::Field(field) => {
                (field.alias.as_deref() == Some("id") && field.name != "id").then_some(field)
            }
            ast::Selection::InlineFragment(fragment) => {
                self.aliased_id(&fragment.selection_set, depth + 1)
            }
            ast::Selection::FragmentSpread(spread) => self
                .fragments
                .get(spread.fragment_name.as_str())
                .and_then(|fragment| self.aliased_id(&fragment.selection_set, depth + 1)),
        })
    }

    /// Refuses an alias Shopify or this crate writes in the lines.
    fn check_key(field: &ast::Field<'_, String>) -> Result<(), Error> {
        if let Some(alias) = &field.alias
            && (alias.starts_with("__") || alias == TYPENAME_KEY)
        {
            return error_at(
                field.position,
                format!(
                    "`{alias}` cannot be an alias in a bulk query: the lines of its results hold `__parentId`, and `{TYPENAME_KEY}` where this crate selects `__typename` to place them"
                ),
            );
        }
        Ok(())
    }

    /// Checks what a `node { … }` selects, and returns its connections.
    fn node_selection(
        &mut self,
        set: &'a SelectionSet<'a>,
        node_type: &str,
        depth: usize,
    ) -> Result<Vec<Child>, Error> {
        let mut children = Vec::new();
        for item in &set.items {
            match item {
                ast::Selection::Field(field) => {
                    Self::check_key(field)?;
                    let Some(definition) = self.field_definition(node_type, field) else {
                        continue;
                    };
                    let ty = definition.ty.clone();
                    if connection_node(self.schema, ty.name()).is_some() {
                        if !matches!(ty.nullable(), TypeRef::Named(_)) {
                            return error_at(
                                field.position,
                                format!(
                                    "`{}` is a list of connections, which a bulk query cannot read",
                                    field.name
                                ),
                            );
                        }
                        let connection = self.connection(field, node_type, depth + 1)?;
                        let node = only_field(&field.selection_set, "edges", "")
                            .and_then(|edges| only_field(&edges.selection_set, "node", ""))
                            .map(|node| node.position)
                            .unwrap_or(field.position);
                        children.push(Child {
                            name: field.name.clone(),
                            position: field.position,
                            node,
                            connection,
                        });
                    } else if !field.selection_set.items.is_empty() {
                        self.plain(&field.selection_set, ty.name(), &field.name, false)?;
                    }
                }
                ast::Selection::InlineFragment(fragment) => {
                    let target = match &fragment.type_condition {
                        Some(ast::TypeCondition::On(name)) => name.as_str(),
                        None => node_type,
                    };
                    self.plain(&fragment.selection_set, target, "", true)?;
                }
                ast::Selection::FragmentSpread(spread) => {
                    if let Some(fragment) = self.fragments.get(spread.fragment_name.as_str()) {
                        let ast::TypeCondition::On(target) = &fragment.type_condition;
                        self.plain(&fragment.selection_set, target, "", true)?;
                    }
                }
            }
        }
        Ok(children)
    }

    /// Checks a selection that may hold no connection: under the field
    /// `under`, or in a fragment.
    fn plain(
        &self,
        set: &'a SelectionSet<'a>,
        parent: &str,
        under: &str,
        fragment: bool,
    ) -> Result<(), Error> {
        for item in &set.items {
            match item {
                ast::Selection::Field(field) => {
                    Self::check_key(field)?;
                    let Some(definition) = self.field_definition(parent, field) else {
                        continue;
                    };
                    let ty = definition.ty.name().to_string();
                    if connection_node(self.schema, &ty).is_some() {
                        let place = if fragment {
                            "inside a fragment".to_string()
                        } else {
                            format!("under `{under}`, which is not a connection")
                        };
                        return error_at(
                            field.position,
                            format!(
                                "The connection `{}` is {place}: in a bulk query, a nested connection is written right in the `node {{ … }}` of its parent connection",
                                field.name
                            ),
                        );
                    }
                    if !field.selection_set.items.is_empty() {
                        let under = if fragment { under } else { &field.name };
                        self.plain(&field.selection_set, &ty, under, fragment)?;
                    }
                }
                ast::Selection::InlineFragment(inline) => {
                    let target = match &inline.type_condition {
                        Some(ast::TypeCondition::On(name)) => name.as_str(),
                        None => parent,
                    };
                    self.plain(&inline.selection_set, target, under, true)?;
                }
                ast::Selection::FragmentSpread(spread) => {
                    if let Some(definition) = self.fragments.get(spread.fragment_name.as_str()) {
                        let ast::TypeCondition::On(target) = &definition.type_condition;
                        self.plain(&definition.selection_set, target, under, true)?;
                    }
                }
            }
        }
        Ok(())
    }
}

/// A nested connection, with where its field and its `node` are.
struct Child {
    name: String,
    position: Pos,
    node: Pos,
    connection: Connection,
}

/// The single field `name` of `set`, without alias, arguments or directives.
fn only_field<'a>(
    set: &'a SelectionSet<'a>,
    name: &str,
    shape: &str,
) -> Result<&'a ast::Field<'a, String>, Error> {
    match set.items.as_slice() {
        [ast::Selection::Field(field)]
            if field.name == name
                && field.alias.is_none()
                && field.arguments.is_empty()
                && field.directives.is_empty() =>
        {
            Ok(field)
        }
        [ast::Selection::Field(field), ..] if field.name == "nodes" => error_at(
            field.position,
            format!("{shape}: Shopify does not document `nodes` for bulk queries"),
        ),
        [item] => error_at(
            item_position(item),
            format!("{shape}: write `{name}`, without alias, arguments or directives"),
        ),
        [first, second, ..] => {
            let other = if matches!(first, ast::Selection::Field(field) if field.name == name) {
                second
            } else {
                first
            };
            error_at(
                item_position(other),
                format!("{shape}: {} is refused beside `{name}`", item_name(other)),
            )
        }
        [] => error_at(set_position(set), shape.to_string()),
    }
}

/// A selection as a message names it.
fn item_name(item: &ast::Selection<'_, String>) -> String {
    match item {
        ast::Selection::Field(field) => format!("`{}`", field.name),
        ast::Selection::InlineFragment(_) | ast::Selection::FragmentSpread(_) => {
            "a fragment".to_string()
        }
    }
}

fn base_name<'t>(ty: &'t ast::Type<'_, String>) -> &'t str {
    match ty {
        ast::Type::NamedType(name) => name,
        ast::Type::ListType(inner) | ast::Type::NonNullType(inner) => base_name(inner),
    }
}

/// A value of input type `name` may hold a `JSON` value.
fn reaches_json<'s>(schema: &'s Schema, name: &'s str, seen: &mut BTreeSet<&'s str>) -> bool {
    if name == "JSON" {
        return true;
    }
    match schema.get(name) {
        Some(TypeDef::Input { fields, .. }) if seen.insert(name) => fields
            .values()
            .any(|field| reaches_json(schema, field.ty.name(), seen)),
        _ => false,
    }
}

/// The selection on the nodes of a connection field of the analyzed operation.
fn connection_nodes(field: &SelectedField) -> &Selection {
    let FieldBase::Object(connection) = &field.base else {
        unreachable!("a connection has subfields");
    };
    let FieldBase::Object(edge) = &connection.fields[0].base else {
        unreachable!("`edges` has subfields");
    };
    let FieldBase::Object(node) = &edge.fields[0].base else {
        unreachable!("`node` has subfields");
    };
    node
}

/// `selection` with each connection made the list of its nodes.
fn item_selection(schema: &Schema, selection: &Selection) -> Selection {
    let mut item = selection.clone();
    for field in &mut item.fields {
        if let Some(node) = connection_node(schema, field.ty.name()) {
            let nodes = item_selection(schema, connection_nodes(field));
            field.ty = TypeRef::NonNull(Box::new(TypeRef::List(Box::new(TypeRef::NonNull(
                Box::new(TypeRef::Named(node.to_string())),
            )))));
            field.base = FieldBase::Object(nodes);
            field.conditional = false;
            field.connection = true;
        }
    }
    item
}

/// A constant value as GraphQL text.
fn constant_text(value: &Value<'_>) -> String {
    match value {
        ast::Value::Variable(name) => format!("${name}"),
        ast::Value::Int(number) => number.as_i64().unwrap_or_default().to_string(),
        // `Debug` keeps a `.` or an exponent: `1.0`, `1e300`.
        ast::Value::Float(number) => format!("{number:?}"),
        ast::Value::String(text) => string_literal(text),
        ast::Value::Boolean(value) => value.to_string(),
        ast::Value::Null => "null".to_string(),
        ast::Value::Enum(name) => name.clone(),
        ast::Value::List(items) => {
            let items: Vec<String> = items.iter().map(constant_text).collect();
            format!("[{}]", items.join(", "))
        }
        ast::Value::Object(fields) => {
            let fields: Vec<String> = fields
                .iter()
                .map(|(name, value)| format!("{name}: {}", constant_text(value)))
                .collect();
            format!("{{{}}}", fields.join(", "))
        }
    }
}

/// A string as a GraphQL string literal: JSON's escapes are GraphQL's.
pub fn string_literal(text: &str) -> String {
    serde_json::to_string(text).expect("a string serializes")
}

fn has_variable(value: &Value<'_>) -> bool {
    match value {
        ast::Value::Variable(_) => true,
        ast::Value::List(items) => items.iter().any(has_variable),
        ast::Value::Object(fields) => fields.values().any(has_variable),
        _ => false,
    }
}

fn value_part(value: &Value<'_>) -> ValuePart {
    match value {
        ast::Value::Variable(name) => ValuePart::Variable(name.clone()),
        ast::Value::List(items) if items.iter().any(has_variable) => {
            ValuePart::List(items.iter().map(value_part).collect())
        }
        ast::Value::Object(fields) if fields.values().any(has_variable) => ValuePart::Object(
            fields
                .iter()
                .map(|(name, value)| Argument {
                    name: name.clone(),
                    value: value_part(value),
                })
                .collect(),
        ),
        constant => ValuePart::Text(constant_text(constant)),
    }
}

/// Writes a document back as text, without its variable definitions, with
/// `__typename` aliased [`TYPENAME_KEY`] where the lines need it.
struct Printer<'c> {
    typename: &'c BTreeSet<(usize, usize)>,
    parts: Vec<Part>,
    text: String,
}

impl Printer<'_> {
    fn finish(mut self) -> Vec<Part> {
        if !self.text.is_empty() {
            self.parts.push(Part::Text(std::mem::take(&mut self.text)));
        }
        self.parts
    }

    fn operation(&mut self, definition: &ast::OperationDefinition<'_, String>) {
        let (name, directives, set) = match definition {
            ast::OperationDefinition::SelectionSet(set) => (None, &[][..], set),
            ast::OperationDefinition::Query(query) => (
                query.name.as_deref(),
                &query.directives[..],
                &query.selection_set,
            ),
            ast::OperationDefinition::Mutation(mutation) => (
                mutation.name.as_deref(),
                &mutation.directives[..],
                &mutation.selection_set,
            ),
            ast::OperationDefinition::Subscription(subscription) => (
                subscription.name.as_deref(),
                &subscription.directives[..],
                &subscription.selection_set,
            ),
        };
        if name.is_some() || !directives.is_empty() {
            self.text.push_str("query");
            if let Some(name) = name {
                self.text.push(' ');
                self.text.push_str(name);
            }
            self.directives(directives);
            self.text.push(' ');
        }
        self.selection_set(set, false);
    }

    fn fragment(&mut self, fragment: &ast::FragmentDefinition<'_, String>) {
        let ast::TypeCondition::On(condition) = &fragment.type_condition;
        self.text
            .push_str(&format!("fragment {} on {condition}", fragment.name));
        self.directives(&fragment.directives);
        self.text.push(' ');
        self.selection_set(&fragment.selection_set, false);
    }

    fn selection_set(&mut self, set: &SelectionSet<'_>, typename: bool) {
        self.text.push('{');
        if typename {
            self.text.push(' ');
            self.text.push_str(TYPENAME_KEY);
            self.text.push_str(": __typename");
        }
        for item in &set.items {
            self.text.push(' ');
            match item {
                ast::Selection::Field(field) => {
                    if let Some(alias) = &field.alias {
                        self.text.push_str(alias);
                        self.text.push_str(": ");
                    }
                    self.text.push_str(&field.name);
                    self.arguments(&field.arguments);
                    self.directives(&field.directives);
                    if !field.selection_set.items.is_empty() {
                        self.text.push(' ');
                        let typename = self
                            .typename
                            .contains(&(field.position.line, field.position.column));
                        self.selection_set(&field.selection_set, typename);
                    }
                }
                ast::Selection::InlineFragment(fragment) => {
                    self.text.push_str("...");
                    if let Some(ast::TypeCondition::On(condition)) = &fragment.type_condition {
                        self.text.push_str(" on ");
                        self.text.push_str(condition);
                    }
                    self.directives(&fragment.directives);
                    self.text.push(' ');
                    self.selection_set(&fragment.selection_set, false);
                }
                ast::Selection::FragmentSpread(spread) => {
                    self.text.push_str("...");
                    self.text.push_str(&spread.fragment_name);
                    self.directives(&spread.directives);
                }
            }
        }
        self.text.push_str(" }");
    }

    fn directives(&mut self, directives: &[ast::Directive<'_, String>]) {
        for directive in directives {
            self.text.push_str(" @");
            self.text.push_str(&directive.name);
            self.arguments(&directive.arguments);
        }
    }

    fn arguments(&mut self, arguments: &[(String, Value<'_>)]) {
        if arguments.is_empty() {
            return;
        }
        if arguments.iter().any(|(_, value)| has_variable(value)) {
            if !self.text.is_empty() {
                self.parts.push(Part::Text(std::mem::take(&mut self.text)));
            }
            self.parts.push(Part::Arguments(
                arguments
                    .iter()
                    .map(|(name, value)| Argument {
                        name: name.clone(),
                        value: value_part(value),
                    })
                    .collect(),
            ));
            return;
        }
        let written: Vec<String> = arguments
            .iter()
            .map(|(name, value)| format!("{name}: {}", constant_text(value)))
            .collect();
        self.text.push('(');
        self.text.push_str(&written.join(", "));
        self.text.push(')');
    }
}

// ─── Code ────────────────────────────────────────────────────────────

/// The name of the `static` that holds the template of a bulk query.
pub fn template_ident() -> proc_macro2::Ident {
    proc_macro2::Ident::new("__SHOPIFY_API_BULK_TEMPLATE", Span::mixed_site())
}

/// The item types of a bulk query, their `BulkItem` implementation, and the
/// `static` [`template_ident`] of its text. `krate` is the path of the
/// `shopify_api` crate in the generated code.
pub fn query_items(
    schema: &Schema,
    operation: &Operation,
    bulk: &BulkQuery,
    krate: &TokenStream,
) -> TokenStream {
    let types = item_types(&bulk.item, ITEM_TYPE, krate);
    let private = quote!(#krate::__private::bulk);
    let item = format_ident!("{}", ITEM_TYPE);

    // Where each line goes.
    let connections = connections_tokens(&bulk.connections, &private);
    let parent = proc_macro2::Ident::new("__shopify_api_parent", Span::mixed_site());
    let slot = proc_macro2::Ident::new("__shopify_api_slot", Span::mixed_site());
    let line = proc_macro2::Ident::new("__shopify_api_line", Span::mixed_site());
    let number = proc_macro2::Ident::new("__shopify_api_number", Span::mixed_site());
    let child = proc_macro2::Ident::new("__shopify_api_child", Span::mixed_site());
    let mut arms = Vec::new();
    let top = field_idents(&bulk.item);
    for (index, connection) in bulk.connections.iter().enumerate() {
        let field = ident_of(&top, &connection.key);
        arms.push(quote! {
            (::core::option::Option::None, #index) => #private::push(&mut self.#field, #line, #number),
        });
        if connection.connections.is_empty() {
            continue;
        }
        let nested = nested_selection(&bulk.item, &connection.key);
        let below = field_idents(nested);
        for (inner, grandchild) in connection.connections.iter().enumerate() {
            let inner_field = ident_of(&below, &grandchild.key);
            arms.push(quote! {
                (::core::option::Option::Some((#index, #child)), #inner) => match self.#field.get_mut(#child) {
                    ::core::option::Option::Some(#child) => #private::push(&mut #child.#inner_field, #line, #number),
                    ::core::option::Option::None => ::core::result::Result::Err(#private::unplaced()),
                },
            });
        }
    }

    let template = template_tokens(schema, operation, bulk, &private);
    let template_ident = template_ident();
    let (parts_ident, variables_ident) = template_parts_idents();
    quote! {
        #types

        impl #private::BulkItem for #item {
            const CONNECTIONS: &'static [#private::BulkConnection] = #connections;

            fn __attach(
                &mut self,
                #parent: ::core::option::Option<(usize, usize)>,
                #slot: usize,
                #line: &[u8],
                #number: usize,
            ) -> #krate::Result<usize> {
                match (#parent, #slot) {
                    #(#arms)*
                    _ => ::core::result::Result::Err(#private::unplaced()),
                }
            }
        }

        #template

        static #template_ident: #private::BulkTemplate = #private::BulkTemplate {
            parts: #parts_ident,
            variables: #variables_ident,
        };
    }
}

fn ident_of<'i>(idents: &'i [(String, proc_macro2::Ident)], key: &str) -> &'i proc_macro2::Ident {
    &idents
        .iter()
        .find(|(field, _)| field == key)
        .expect("a connection is a field of its parent")
        .1
}

/// The selection on the nodes of the connection `key` of `item`.
fn nested_selection<'s>(item: &'s Selection, key: &str) -> &'s Selection {
    match item
        .fields
        .iter()
        .find(|field| field.key == key)
        .map(|field| &field.base)
    {
        Some(FieldBase::Object(selection)) => selection,
        _ => unreachable!("a connection has subfields"),
    }
}

fn connections_tokens(connections: &[Connection], private: &TokenStream) -> TokenStream {
    let items = connections.iter().map(|connection| {
        let key = &connection.key;
        let types = &connection.types;
        let nested = connections_tokens(&connection.connections, private);
        quote! {
            #private::BulkConnection {
                key: #key,
                types: &[#(#types),*],
                connections: #nested,
            }
        }
    });
    quote!(&[#(#items),*])
}

/// The parts and the variables of the text, as `static`s named
/// `__SHOPIFY_API_BULK_PARTS` and `__SHOPIFY_API_BULK_VARIABLES`, with one
/// `static` per input object whose fields hold enums.
fn template_tokens(
    schema: &Schema,
    operation: &Operation,
    bulk: &BulkQuery,
    private: &TokenStream,
) -> TokenStream {
    let names: Vec<&str> = operation
        .variables
        .iter()
        .map(|variable| variable.name.as_str())
        .collect();
    let parts = bulk.parts.iter().map(|part| match part {
        Part::Text(text) => quote!(#private::BulkPart::Text(#text)),
        Part::Arguments(arguments) => {
            let arguments = arguments_tokens(arguments, &names, private);
            quote!(#private::BulkPart::Arguments(#arguments))
        }
    });
    let mut inputs = BTreeMap::new();
    let variables: Vec<TokenStream> = operation
        .variables
        .iter()
        .map(|variable| {
            let name = &variable.name;
            let ty = type_tokens(schema, &variable.ty, private, &mut inputs);
            let default = match bulk.defaults.get(name) {
                Some(text) => quote!(::core::option::Option::Some(#text)),
                None => quote!(::core::option::Option::None),
            };
            quote! {
                #private::BulkVariable { name: #name, ty: #ty, default: #default }
            }
        })
        .collect();
    let statics = inputs.into_iter().filter_map(|(name, fields)| {
        let fields = fields?;
        let ident = input_ident(&name);
        let count = fields.len();
        let fields = fields.into_iter().map(|(field, ty)| quote!((#field, #ty)));
        Some(quote! {
            static #ident: [(&str, #private::BulkType); #count] = [#(#fields),*];
        })
    });
    let (parts_ident, variables_ident) = template_parts_idents();
    quote! {
        #(#statics)*
        static #parts_ident: &[#private::BulkPart] = &[#(#parts),*];
        static #variables_ident: &[#private::BulkVariable] = &[#(#variables),*];
    }
}

/// The `static`s of the parts and of the variables of the text.
fn template_parts_idents() -> (proc_macro2::Ident, proc_macro2::Ident) {
    (
        proc_macro2::Ident::new("__SHOPIFY_API_BULK_PARTS", Span::mixed_site()),
        proc_macro2::Ident::new("__SHOPIFY_API_BULK_VARIABLES", Span::mixed_site()),
    )
}

fn input_ident(name: &str) -> proc_macro2::Ident {
    proc_macro2::Ident::new(
        &format!("__SHOPIFY_API_BULK_INPUT_{name}"),
        Span::mixed_site(),
    )
}

fn arguments_tokens(arguments: &[Argument], names: &[&str], private: &TokenStream) -> TokenStream {
    let items = arguments.iter().map(|argument| {
        let name = &argument.name;
        let value = value_tokens(&argument.value, names, private);
        quote!(#private::BulkArgument { name: #name, value: #value })
    });
    quote!(&[#(#items),*])
}

fn value_tokens(value: &ValuePart, names: &[&str], private: &TokenStream) -> TokenStream {
    match value {
        ValuePart::Text(text) => quote!(#private::BulkValue::Text(#text)),
        ValuePart::Variable(name) => {
            let index = names
                .iter()
                .position(|declared| declared == name)
                .expect("`analyze` checked that the variable is declared");
            quote!(#private::BulkValue::Variable(#index))
        }
        ValuePart::List(items) => {
            let items = items.iter().map(|item| value_tokens(item, names, private));
            quote!(#private::BulkValue::List(&[#(#items),*]))
        }
        ValuePart::Object(fields) => {
            let fields = arguments_tokens(fields, names, private);
            quote!(#private::BulkValue::Object(#fields))
        }
    }
}

/// Input objects by name: the fields that hold enums, with their type, or
/// `None` while they are being written.
type Inputs = BTreeMap<String, Option<Vec<(String, TokenStream)>>>;

/// Where the enums of a value of type `ty` are, which are written bare.
fn type_tokens(
    schema: &Schema,
    ty: &TypeRef,
    private: &TokenStream,
    inputs: &mut Inputs,
) -> TokenStream {
    match ty {
        TypeRef::NonNull(inner) => type_tokens(schema, inner, private, inputs),
        TypeRef::List(item) => {
            let item = type_tokens(schema, item, private, inputs);
            quote!(#private::BulkType::List(&#item))
        }
        TypeRef::Named(name) => match schema.get(name) {
            Some(TypeDef::Enum { .. }) => quote!(#private::BulkType::Enum),
            Some(TypeDef::Input { fields, .. })
                if reaches_enum(schema, name, &mut BTreeSet::new()) =>
            {
                if !inputs.contains_key(name) {
                    inputs.insert(name.clone(), None);
                    let written = fields
                        .iter()
                        .filter(|(_, field)| {
                            reaches_enum(schema, field.ty.name(), &mut BTreeSet::new())
                        })
                        .map(|(field, input)| {
                            (
                                field.clone(),
                                type_tokens(schema, &input.ty, private, inputs),
                            )
                        })
                        .collect();
                    inputs.insert(name.clone(), Some(written));
                }
                let ident = input_ident(name);
                quote!(#private::BulkType::Input(&#ident))
            }
            _ => quote!(#private::BulkType::Plain),
        },
    }
}

/// A value of input type `name` may hold an enum value.
fn reaches_enum<'s>(schema: &'s Schema, name: &'s str, seen: &mut BTreeSet<&'s str>) -> bool {
    match schema.get(name) {
        Some(TypeDef::Enum { .. }) => true,
        Some(TypeDef::Input { fields, .. }) if seen.insert(name) => fields
            .values()
            .any(|field| reaches_enum(schema, field.ty.name(), seen)),
        _ => false,
    }
}
