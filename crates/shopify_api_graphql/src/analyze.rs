//! Checks a GraphQL operation against a schema and describes the shape of
//! its variables and of its result.
//!
//! # Stricter than the specification, on purpose
//!
//! Where Shopify's server (GraphQL Ruby) refuses what the specification
//! accepts, the analyzer refuses it too, so that an operation it accepts is
//! one Shopify runs. One response key selected twice must have the fields of
//! each input object of its arguments in the same order: the specification
//! says input fields are unordered (`{a: 1, b: 2}` is `{b: 2, a: 1}`), but
//! GraphQL Ruby compares their text and answers `fieldConflict`. An alias
//! spelled like its field (`draftOrderCreate: draftOrderCreate(...)`) is an
//! alias all the same. When the order cannot be read, the operation is
//! refused rather than guessed.

mod input_objects;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

use graphql_parser::Pos;
use graphql_parser::query as ast;

use crate::schema::{Field, InputValue, Schema, TypeDef, TypeRef};

type Document<'a> = ast::Document<'a, String>;
type SelectionSet<'a> = ast::SelectionSet<'a, String>;
type Value<'a> = ast::Value<'a, String>;
type Directive<'a> = ast::Directive<'a, String>;

/// An operation that does not parse or does not fit the schema.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Error {
    pub message: String,
    /// Line and column in the operation text, both starting at 1.
    pub position: Option<(usize, usize)>,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.position {
            Some((line, column)) => {
                write!(formatter, "{} (line {line}, column {column})", self.message)
            }
            None => formatter.write_str(&self.message),
        }
    }
}

impl std::error::Error for Error {}

type Result<T> = std::result::Result<T, Error>;

fn error_at<T>(position: Pos, message: impl Into<String>) -> Result<T> {
    Err(Error {
        message: message.into(),
        position: Some((position.line, position.column)),
    })
}

fn error<T>(message: impl Into<String>) -> Result<T> {
    Err(Error {
        message: message.into(),
        position: None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OperationKind {
    Query,
    Mutation,
}

/// How a scalar travels in JSON.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Scalar {
    /// `String`, `ID`, enum values, and Shopify's string-encoded scalars such
    /// as `DateTime`, `Decimal`, `Money`, `URL` and `UnsignedInt64`.
    String,
    Int,
    Float,
    Boolean,
    /// `JSON`: any value.
    Json,
}

impl Scalar {
    /// How the scalar, or the enum, of this name travels.
    pub(crate) fn of(name: &str) -> Self {
        match name {
            "Int" => Self::Int,
            "Float" => Self::Float,
            "Boolean" => Self::Boolean,
            "JSON" => Self::Json,
            _ => Self::String,
        }
    }
}

/// What a variable holds, lists and nullability set aside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum VariableBase {
    Scalar(Scalar),
    Enum,
    /// An input object, named by the variable's type.
    Input,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Variable {
    pub name: String,
    pub ty: TypeRef,
    pub has_default: bool,
    pub base: VariableBase,
}

impl Variable {
    /// The operation cannot run without a value for this variable.
    pub fn is_required(&self) -> bool {
        self.ty.is_non_null() && !self.has_default
    }
}

/// The fields selected on an object, interface or union.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Selection {
    /// The type the fields are selected on: of the first place, when one
    /// response key names fields of different types (see `types`).
    pub type_name: String,
    pub fields: Vec<SelectedField>,
    /// Every type the fields are selected on, `type_name` first. More than
    /// one when a response key names fields of different object, interface or
    /// union types below two exclusive object types (`price` is
    /// `Product.priceRangeV2` and `Order.totalPriceSet`): the fields are then
    /// those of every place, each conditional unless every place selects it.
    pub types: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SelectedField {
    /// The key of the field in the response: its alias, or its name.
    pub key: String,
    pub ty: TypeRef,
    /// The field may be absent: it is selected in a fragment that does not
    /// always apply, or under `@include` / `@skip`.
    pub conditional: bool,
    pub base: FieldBase,
    /// A connection of a bulk query, read as the list of its nodes: absent
    /// from the line of its parent, filled from the lines that follow.
    pub connection: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum FieldBase {
    Scalar(Scalar),
    Object(Selection),
}

/// A deprecated field, argument, input field or enum value that an
/// operation uses. `graphql!` turns each one into a compiler warning.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Deprecation {
    /// What is deprecated: `Product.bodyHtml`, `Collection.image(maxWidth:)`,
    /// `CollectionInput.publications` or `OrderSortKeys.CREATED_AT`.
    pub coordinate: String,
    /// Shopify's reason, which usually names what replaces it.
    pub reason: String,
    /// Line and column in the operation text, both starting at 1.
    pub position: (usize, usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Operation {
    pub kind: OperationKind,
    pub name: Option<String>,
    pub variables: Vec<Variable>,
    pub data: Selection,
    /// What the operation uses that the schema deprecates, in the order of the text.
    pub deprecations: Vec<Deprecation>,
}

/// Parses `source`, after refusing the block strings the parser panics on.
pub(crate) fn parse(source: &str) -> Result<Document<'_>> {
    if let Some(position) = crate::source::unreadable_block_strings(source) {
        return error_at(
            position,
            "This block string indents its lines with different Unicode whitespace, which the GraphQL parser this crate uses cannot read. Indent it with spaces or tabs, or write it as a quoted string",
        );
    }
    graphql_parser::parse_query(source).or_else(|parse_error| {
        let text = parse_error.to_string();
        let detail = text.strip_prefix("query parse error: ").unwrap_or(&text);
        error(format!(
            "Invalid GraphQL: {}",
            detail.trim().replace('\n', " ")
        ))
    })
}

/// Parses `source`, checks it against `schema` and returns its shape.
///
/// `source` contains exactly one query or mutation, and the fragments it uses.
pub fn analyze(schema: &Schema, source: &str) -> Result<Operation> {
    let document = parse(source)?;

    // Checked on the text: the parser keeps only the last of the two.
    if let Some((field, position)) = input_objects::duplicate_field(source) {
        return error_at(
            position,
            format!("The input field `{field}` is given twice in the same input object"),
        );
    }

    let mut operations = Vec::new();
    let mut fragments = BTreeMap::new();
    for definition in &document.definitions {
        match definition {
            ast::Definition::Operation(operation) => operations.push(operation),
            ast::Definition::Fragment(fragment) => {
                if fragments.insert(fragment.name.as_str(), fragment).is_some() {
                    return error_at(
                        fragment.position,
                        format!("Fragment `{}` is defined twice", fragment.name),
                    );
                }
            }
        }
    }
    let operation = match operations.as_slice() {
        [operation] => *operation,
        [] => return error("The document contains no query or mutation"),
        [_, second, ..] => {
            return error_at(
                operation_position(second),
                "The document contains several operations; `graphql!` takes one",
            );
        }
    };

    let (kind, name, definitions, directives, selection_set, position) = match operation {
        ast::OperationDefinition::SelectionSet(set) => (
            OperationKind::Query,
            None,
            &[][..],
            &[][..],
            set,
            set.span.0,
        ),
        ast::OperationDefinition::Query(query) => (
            OperationKind::Query,
            query.name.clone(),
            &query.variable_definitions[..],
            &query.directives[..],
            &query.selection_set,
            query.position,
        ),
        ast::OperationDefinition::Mutation(mutation) => (
            OperationKind::Mutation,
            mutation.name.clone(),
            &mutation.variable_definitions[..],
            &mutation.directives[..],
            &mutation.selection_set,
            mutation.position,
        ),
        ast::OperationDefinition::Subscription(subscription) => {
            return error_at(subscription.position, "Subscriptions are not supported");
        }
    };
    let root = match kind {
        OperationKind::Query => schema.query.as_str(),
        OperationKind::Mutation => match &schema.mutation {
            Some(mutation) => mutation.as_str(),
            None => return error_at(position, "The schema has no mutations"),
        },
    };

    let mut analyzer = Analyzer {
        schema,
        source,
        argument_orders: HashMap::new(),
        fragments,
        variables: Vec::new(),
        used_variables: BTreeSet::new(),
        null_defaults: BTreeSet::new(),
        used_fragments: BTreeSet::new(),
        fragment_stack: Vec::new(),
        deprecations: Vec::new(),
    };

    for definition in definitions {
        let ty = type_ref(&definition.var_type);
        let base = match schema.get(ty.name()) {
            Some(TypeDef::Scalar) => VariableBase::Scalar(Scalar::of(ty.name())),
            Some(TypeDef::Enum { .. }) => VariableBase::Enum,
            Some(TypeDef::Input { .. }) => VariableBase::Input,
            Some(other) => {
                return error_at(
                    definition.position,
                    format!(
                        "Variable `${}` cannot have the type `{ty}`: `{}` is {}, not an input type",
                        definition.name,
                        ty.name(),
                        other.kind_name()
                    ),
                );
            }
            None => {
                return error_at(
                    definition.position,
                    format!(
                        "Variable `${}` has the unknown type `{}`{}",
                        definition.name,
                        ty.name(),
                        did_you_mean(ty.name(), schema.types.keys().map(String::as_str))
                    ),
                );
            }
        };
        if analyzer
            .variables
            .iter()
            .any(|declared| declared.name == definition.name)
        {
            return error_at(
                definition.position,
                format!("Variable `${}` is declared twice", definition.name),
            );
        }
        analyzer.variables.push(Variable {
            name: definition.name.clone(),
            ty: ty.clone(),
            has_default: definition.default_value.is_some(),
            base,
        });
        if let Some(default) = &definition.default_value {
            if matches!(default, ast::Value::Null) {
                analyzer.null_defaults.insert(definition.name.clone());
            }
            // The parser refuses a variable here.
            let expected = InputValue::of(ty);
            let place = format!("the default value of `${}`", definition.name);
            analyzer.check_value(default, &expected, &place, definition.position)?;
        }
    }

    let location = match kind {
        OperationKind::Query => "QUERY",
        OperationKind::Mutation => "MUTATION",
    };
    analyzer.check_directives(directives, location)?;
    let data = analyzer.selection(
        &[SubSet {
            set: selection_set,
            conditional: false,
            lineage: Vec::new(),
            parent: root.to_string(),
        }],
        position,
    )?;

    if let Some(unused) = analyzer
        .variables
        .iter()
        .find(|variable| !analyzer.used_variables.contains(&variable.name))
    {
        return error(format!(
            "Variable `${}` is declared but never used",
            unused.name
        ));
    }
    if let Some((unused, fragment)) = analyzer
        .fragments
        .iter()
        .find(|(name, _)| !analyzer.used_fragments.contains(**name))
    {
        return error_at(
            fragment.position,
            format!("Fragment `{unused}` is defined but never used"),
        );
    }

    let mut deprecations = analyzer.deprecations;
    deprecations.sort_by(|left, right| {
        (left.position, &left.coordinate).cmp(&(right.position, &right.coordinate))
    });
    deprecations.dedup();
    Ok(Operation {
        kind,
        name,
        variables: analyzer.variables,
        data,
        deprecations,
    })
}

fn operation_position(operation: &ast::OperationDefinition<'_, String>) -> Pos {
    match operation {
        ast::OperationDefinition::SelectionSet(set) => set.span.0,
        ast::OperationDefinition::Query(query) => query.position,
        ast::OperationDefinition::Mutation(mutation) => mutation.position,
        ast::OperationDefinition::Subscription(subscription) => subscription.position,
    }
}

fn type_ref(ty: &ast::Type<'_, String>) -> TypeRef {
    match ty {
        ast::Type::NamedType(name) => TypeRef::Named(name.clone()),
        ast::Type::ListType(inner) => TypeRef::List(Box::new(type_ref(inner))),
        ast::Type::NonNullType(inner) => TypeRef::NonNull(Box::new(type_ref(inner))),
    }
}

/// A field being assembled from every place that selects its response key.
struct Pending<'a> {
    key: String,
    field: String,
    ty: TypeRef,
    /// Where the key was selected: on which parent type, which field, with
    /// which arguments.
    origins: Vec<Origin<'a>>,
    /// The field may be absent from a value of the parent.
    conditional: bool,
    /// The subfields selected at each place that selects some.
    sets: Vec<SubSet<'a>>,
    /// The selections of the parent (see [`Analyzer::selection`]) that select
    /// the field unconditionally.
    unconditional_in: BTreeSet<usize>,
    position: Pos,
}

/// The selection set of a field at one place.
struct SubSet<'a> {
    set: &'a SelectionSet<'a>,
    /// The place may be absent while the value is present.
    conditional: bool,
    /// The [`Lineage`] of the place.
    lineage: Lineage,
    /// The type the set is selected on: the field's type at that place, which
    /// may differ from one place to another (see [`Selection::types`]).
    parent: String,
}

/// The parent type of a place and of each field above it, from the root:
/// two places whose lineages differ by two object types at some depth are
/// never both in one answer.
type Lineage = Vec<String>;

/// One place a response key is selected at.
struct Origin<'a> {
    /// Its parent type last.
    lineage: Lineage,
    field: String,
    /// The type of the field there.
    ty: TypeRef,
    arguments: &'a [(String, Value<'a>)],
    position: Pos,
    /// The field has a selection set there.
    selects: bool,
}

/// How the arguments of two places that select one response key compare.
enum Arguments {
    Same,
    Different,
    /// Equal, but the fields of an input object come in another order.
    Reordered,
}

/// Which of the sets merged into one selection a field comes from.
#[derive(Clone, Copy)]
struct Place {
    index: usize,
    /// The set may be absent while the value is present.
    uncertain: bool,
}

/// Two values written alike, as Shopify compares the arguments of two fields
/// (GraphQL Ruby's `fieldConflict`): of the same kind (`1` is not `1.0`, `"A"`
/// is not `A`), equal, with lists in the same order. Input objects are
/// compared field by field here; the order of their fields, which the parser
/// does not keep, is compared on the text (see [`Analyzer::same_arguments`]).
fn same_value(left: &Value<'_>, right: &Value<'_>) -> bool {
    use ast::Value as V;
    match (left, right) {
        (V::Variable(left), V::Variable(right))
        | (V::String(left), V::String(right))
        | (V::Enum(left), V::Enum(right)) => left == right,
        (V::Int(left), V::Int(right)) => left.as_i64() == right.as_i64(),
        // `-0.0` is not `0.0`; `1.0` is `1.00`.
        (V::Float(left), V::Float(right)) => left.to_bits() == right.to_bits(),
        (V::Boolean(left), V::Boolean(right)) => left == right,
        (V::Null, V::Null) => true,
        (V::List(left), V::List(right)) => {
            left.len() == right.len()
                && left
                    .iter()
                    .zip(right)
                    .all(|(left, right)| same_value(left, right))
        }
        (V::Object(left), V::Object(right)) => {
            left.len() == right.len()
                && left.iter().all(|(name, left)| {
                    right.get(name).is_some_and(|right| same_value(left, right))
                })
        }
        _ => false,
    }
}

/// Holds an input object of two fields or more, whose order matters.
fn has_ordered_object(value: &Value<'_>) -> bool {
    match value {
        ast::Value::Object(fields) => fields.len() > 1 || fields.values().any(has_ordered_object),
        ast::Value::List(items) => items.iter().any(has_ordered_object),
        _ => false,
    }
}

/// The arguments of a field for a message, each value written with its kind.
fn arguments_text(arguments: &[(String, Value<'_>)]) -> String {
    let written: Vec<String> = arguments
        .iter()
        .map(|(name, value)| format!("{name}: {}", value_text(value)))
        .collect();
    written.join(", ")
}

fn value_text(value: &Value<'_>) -> String {
    match value {
        ast::Value::Variable(name) => format!("${name}"),
        ast::Value::Int(number) => number.as_i64().unwrap_or_default().to_string(),
        ast::Value::Float(number) => format!("{number:?}"),
        ast::Value::String(text) => format!("{text:?}"),
        ast::Value::Boolean(value) => value.to_string(),
        ast::Value::Null => "null".to_string(),
        ast::Value::Enum(name) => name.clone(),
        ast::Value::List(items) => {
            let items: Vec<String> = items.iter().map(value_text).collect();
            format!("[{}]", items.join(", "))
        }
        ast::Value::Object(fields) => {
            let fields: Vec<String> = fields
                .iter()
                .map(|(name, value)| format!("{name}: {}", value_text(value)))
                .collect();
            format!("{{{}}}", fields.join(", "))
        }
    }
}

/// The arguments of a field and the order of their input fields, by name.
type ArgumentOrders<'a> = Option<Vec<(&'a str, Vec<&'a str>)>>;

struct Analyzer<'a> {
    schema: &'a Schema,
    source: &'a str,
    /// [`input_objects::argument_orders`] of the fields compared so far, by
    /// position.
    argument_orders: HashMap<(usize, usize), ArgumentOrders<'a>>,
    fragments: BTreeMap<&'a str, &'a ast::FragmentDefinition<'a, String>>,
    /// The declared variables, in the order of the operation.
    variables: Vec<Variable>,
    used_variables: BTreeSet<String>,
    /// The variables whose default value is `null`: it does not stand in for
    /// a value where `null` is not allowed.
    null_defaults: BTreeSet<String>,
    used_fragments: BTreeSet<&'a str>,
    fragment_stack: Vec<&'a str>,
    deprecations: Vec<Deprecation>,
}

impl<'a> Analyzer<'a> {
    fn deprecated(&mut self, coordinate: String, reason: Option<&String>, position: Pos) {
        if let Some(reason) = reason {
            self.deprecations.push(Deprecation {
                coordinate,
                reason: reason.clone(),
                position: (position.line, position.column),
            });
        }
    }

    /// Resolves the merged selection sets of one value of type `parent`.
    ///
    /// Each set comes with whether the place that selected it may be absent
    /// while the value is present. When one place is always there, what the
    /// others select is conditional. When none is, any of them may be the
    /// one that is there: a subfield is then certain only if every place
    /// selects it.
    fn selection(&mut self, sets: &[SubSet<'a>], position: Pos) -> Result<Selection> {
        let mut types: Vec<String> = Vec::new();
        for sub in sets {
            if !types.contains(&sub.parent) {
                types.push(sub.parent.clone());
            }
        }
        let any_certain = sets.iter().any(|sub| !sub.conditional);
        let mut pending = Vec::new();
        for (index, sub) in sets.iter().enumerate() {
            let place = Place {
                index,
                // Several places that may each be the one there: what each
                // brings below its fields is only there with it.
                uncertain: !any_certain && sets.len() > 1,
            };
            self.collect(
                &sub.parent,
                sub.set,
                any_certain && sub.conditional,
                place,
                &sub.lineage,
                &mut pending,
            )?;
        }
        if !any_certain && sets.len() > 1 {
            for field in &mut pending {
                field.conditional |= field.unconditional_in.len() < sets.len();
            }
        }
        if pending.is_empty() {
            return error_at(
                position,
                format!("The selection on `{}` is empty", types.join("` and `")),
            );
        }

        let mut fields = Vec::new();
        for field in pending {
            let named = field.ty.name();
            let base = match self.schema.get(named) {
                Some(definition) if definition.is_composite() => {
                    // Every place selects subfields, not only one of them.
                    if let Some(bare) = field.origins.iter().find(|origin| !origin.selects) {
                        return error_at(
                            bare.position,
                            format!(
                                "Field `{}` has the type `{}`: select the subfields you need, `{} {{ … }}`",
                                bare.field, bare.ty, bare.field
                            ),
                        );
                    }
                    FieldBase::Object(self.selection(&field.sets, field.position)?)
                }
                Some(_) => {
                    if let Some(selecting) = field.origins.iter().find(|origin| origin.selects) {
                        return error_at(
                            selecting.position,
                            format!(
                                "Field `{}` has the type `{}`, which has no subfields",
                                selecting.field, selecting.ty
                            ),
                        );
                    }
                    FieldBase::Scalar(Scalar::of(named))
                }
                None => {
                    return error_at(
                        field.position,
                        format!("The schema does not define the type `{named}`"),
                    );
                }
            };
            fields.push(SelectedField {
                key: field.key,
                ty: field.ty,
                conditional: field.conditional,
                base,
                connection: false,
            });
        }

        Ok(Selection {
            type_name: types[0].clone(),
            fields,
            types,
        })
    }

    /// Collects the fields `set` selects on `parent`, from `place` among
    /// the sets [`Analyzer::selection`] merges.
    fn collect(
        &mut self,
        parent: &str,
        set: &'a SelectionSet<'a>,
        conditional: bool,
        place: Place,
        lineage: &[String],
        pending: &mut Vec<Pending<'a>>,
    ) -> Result<()> {
        for item in &set.items {
            match item {
                ast::Selection::Field(field) => {
                    let conditional =
                        conditional | self.check_directives(&field.directives, "FIELD")?;
                    let ty = self.field_type(parent, field)?;
                    let key = field.alias.as_ref().unwrap_or(&field.name).clone();
                    let selects = !field.selection_set.items.is_empty();
                    let origin = |lineage| Origin {
                        lineage,
                        field: field.name.clone(),
                        ty: ty.clone(),
                        arguments: &field.arguments,
                        position: field.position,
                        selects,
                    };
                    let origin_lineage: Lineage = lineage
                        .iter()
                        .cloned()
                        .chain([parent.to_string()])
                        .collect();
                    let sets = selects.then(|| SubSet {
                        set: &field.selection_set,
                        conditional: conditional || place.uncertain,
                        lineage: origin_lineage.clone(),
                        parent: ty.name().to_string(),
                    });

                    match pending.iter_mut().find(|existing| existing.key == key) {
                        Some(existing) => {
                            // SameResponseShape: the answer holds one value
                            // under the key, of one shape. Below, the
                            // subfields of every place merge and are checked
                            // the same way.
                            if !self.same_shape(&existing.ty, &ty) {
                                return error_at(
                                    field.position,
                                    format!(
                                        "`{key}` is selected twice with different fields or types (`{}: {}` and `{}: {ty}`); give one of them an alias",
                                        existing.field, existing.ty, field.name
                                    ),
                                );
                            }
                            // FieldsInSetCanMerge: on two different object
                            // types, never both there, the fields and their
                            // arguments may differ, and so may those below
                            // them; elsewhere they may not.
                            for origin in &existing.origins {
                                if !self.exclusive(&origin.lineage, &origin_lineage) {
                                    if origin.field != field.name {
                                        return error_at(
                                            field.position,
                                            format!(
                                                "`{key}` is selected twice with different fields or types (`{}: {}` and `{}: {ty}`); give one of them an alias",
                                                origin.field, existing.ty, field.name
                                            ),
                                        );
                                    }
                                    match self.same_arguments(origin, field) {
                                        Arguments::Same => {}
                                        Arguments::Different => {
                                            return error_at(
                                                field.position,
                                                format!(
                                                    "`{key}` is selected twice with different arguments (`({})` and `({})`); give one of them an alias",
                                                    arguments_text(origin.arguments),
                                                    arguments_text(&field.arguments)
                                                ),
                                            );
                                        }
                                        Arguments::Reordered => {
                                            return error_at(
                                                field.position,
                                                format!(
                                                    "`{key}` is selected twice with the fields of an input object in another order (`({})`), which Shopify refuses; write them in the same order, or give one of them an alias",
                                                    arguments_text(&field.arguments)
                                                ),
                                            );
                                        }
                                    }
                                }
                            }
                            existing.origins.push(origin(origin_lineage));
                            existing.conditional &= conditional;
                            existing.sets.extend(sets);
                            if !conditional {
                                existing.unconditional_in.insert(place.index);
                            }
                        }
                        None => pending.push(Pending {
                            key,
                            field: field.name.clone(),
                            ty: ty.clone(),
                            origins: vec![origin(origin_lineage)],
                            conditional,
                            sets: sets.into_iter().collect(),
                            unconditional_in: (!conditional)
                                .then_some(place.index)
                                .into_iter()
                                .collect(),
                            position: field.position,
                        }),
                    }
                }
                ast::Selection::InlineFragment(fragment) => {
                    let conditional = conditional
                        | self.check_directives(&fragment.directives, "INLINE_FRAGMENT")?;
                    let (target, always) = match &fragment.type_condition {
                        Some(ast::TypeCondition::On(condition)) => (
                            condition.as_str(),
                            self.fragment_always_applies(parent, condition, fragment.position)?,
                        ),
                        None => (parent, true),
                    };
                    self.collect(
                        target,
                        &fragment.selection_set,
                        conditional | !always,
                        place,
                        lineage,
                        pending,
                    )?;
                }
                ast::Selection::FragmentSpread(spread) => {
                    let conditional = conditional
                        | self.check_directives(&spread.directives, "FRAGMENT_SPREAD")?;
                    let name = spread.fragment_name.as_str();
                    let Some(fragment) = self.fragments.get(name).copied() else {
                        return error_at(
                            spread.position,
                            format!(
                                "Fragment `{name}` is not defined{}; a `graphql!` document contains the fragments it uses",
                                did_you_mean(name, self.fragments.keys().copied())
                            ),
                        );
                    };
                    if self.fragment_stack.contains(&name) {
                        return error_at(
                            spread.position,
                            format!("Fragment `{name}` includes itself"),
                        );
                    }
                    self.used_fragments.insert(fragment.name.as_str());
                    self.check_directives(&fragment.directives, "FRAGMENT_DEFINITION")?;

                    let ast::TypeCondition::On(condition) = &fragment.type_condition;
                    let always =
                        self.fragment_always_applies(parent, condition, spread.position)?;
                    self.fragment_stack.push(fragment.name.as_str());
                    let collected = self.collect(
                        condition,
                        &fragment.selection_set,
                        conditional | !always,
                        place,
                        lineage,
                        pending,
                    );
                    self.fragment_stack.pop();
                    collected?;
                }
            }
        }
        Ok(())
    }

    /// Whether `field` gives the arguments `origin` gave.
    fn same_arguments(
        &mut self,
        origin: &Origin<'a>,
        field: &'a ast::Field<'a, String>,
    ) -> Arguments {
        let (left, right) = (origin.arguments, &field.arguments);
        let same = left.len() == right.len()
            && left.iter().all(|(name, left)| {
                right
                    .iter()
                    .find(|(given, _)| given == name)
                    .is_some_and(|(_, right)| same_value(left, right))
            });
        if !same {
            return Arguments::Different;
        }
        if !left.iter().any(|(_, value)| has_ordered_object(value)) {
            return Arguments::Same;
        }
        let left = self.argument_orders(origin.position, &origin.field);
        let right = self.argument_orders(field.position, &field.name);
        match (left, right) {
            (Some(left), Some(right))
                if left.iter().all(|(name, order)| {
                    right
                        .iter()
                        .any(|(given, other)| given == name && order == other)
                }) =>
            {
                Arguments::Same
            }
            // The text cannot be read back: refuse rather than send what
            // Shopify may refuse.
            _ => Arguments::Reordered,
        }
    }

    fn argument_orders(&mut self, position: Pos, name: &str) -> ArgumentOrders<'a> {
        let source = self.source;
        self.argument_orders
            .entry((position.line, position.column))
            .or_insert_with(|| input_objects::argument_orders(source, position, name))
            .clone()
    }

    /// SameResponseShape, for the types of two fields: the same lists and
    /// nullability, the same scalar or enum, or two types with subfields,
    /// whose selections [`Analyzer::selection`] then merges.
    fn same_shape(&self, left: &TypeRef, right: &TypeRef) -> bool {
        match (left, right) {
            (TypeRef::NonNull(left), TypeRef::NonNull(right))
            | (TypeRef::List(left), TypeRef::List(right)) => self.same_shape(left, right),
            (TypeRef::Named(left), TypeRef::Named(right)) => {
                let composite =
                    |name: &str| self.schema.get(name).is_some_and(TypeDef::is_composite);
                left == right || (composite(left) && composite(right))
            }
            _ => false,
        }
    }

    /// An object type, as opposed to an interface or a union.
    fn is_object(&self, name: &str) -> bool {
        matches!(self.schema.get(name), Some(TypeDef::Object { .. }))
    }

    /// Two places never both in one answer: at some depth, their parents are
    /// two different object types.
    fn exclusive(&self, left: &[String], right: &[String]) -> bool {
        left.iter()
            .zip(right)
            .any(|(left, right)| left != right && self.is_object(left) && self.is_object(right))
    }

    /// Whether every value of type `parent` matches a fragment on `condition`.
    /// Fails when none can.
    fn fragment_always_applies(
        &self,
        parent: &str,
        condition: &str,
        position: Pos,
    ) -> Result<bool> {
        match self.schema.get(condition) {
            Some(definition) if definition.is_composite() => {}
            Some(definition) => {
                return error_at(
                    position,
                    format!(
                        "A fragment cannot be on `{condition}`, which is {}",
                        definition.kind_name()
                    ),
                );
            }
            None => {
                return error_at(
                    position,
                    format!(
                        "A fragment is on the unknown type `{condition}`{}",
                        did_you_mean(condition, self.schema.possible_types(parent).into_iter())
                    ),
                );
            }
        }
        let of_parent = self.schema.possible_types(parent);
        let of_condition = self.schema.possible_types(condition);
        if !of_parent.iter().any(|ty| of_condition.contains(ty)) {
            return error_at(
                position,
                format!("A fragment on `{condition}` never applies to a `{parent}`"),
            );
        }
        Ok(of_parent.iter().all(|ty| of_condition.contains(ty)))
    }

    /// The type of `field` on `parent`, after checking its arguments.
    fn field_type(&mut self, parent: &str, field: &'a ast::Field<'a, String>) -> Result<TypeRef> {
        if field.name == "__typename" {
            if let Some((argument, _)) = field.arguments.first() {
                return error_at(
                    field.position,
                    format!("Field `__typename` has no argument `{argument}`"),
                );
            }
            return Ok(TypeRef::NonNull(Box::new(TypeRef::Named(
                "String".to_string(),
            ))));
        }

        let fields = match self.schema.get(parent) {
            Some(TypeDef::Object { fields, .. } | TypeDef::Interface { fields, .. }) => fields,
            Some(TypeDef::Union { possible_types }) => {
                return error_at(
                    field.position,
                    format!(
                        "`{parent}` is a union: select `{}` inside a fragment such as `... on {} {{ {} }}`",
                        field.name,
                        possible_types.first().map_or("Type", String::as_str),
                        field.name
                    ),
                );
            }
            _ => {
                return error_at(
                    field.position,
                    format!("`{parent}` has no fields to select"),
                );
            }
        };
        let Some(definition) = fields.get(&field.name) else {
            return error_at(
                field.position,
                format!(
                    "Type `{parent}` has no field `{}`{}",
                    field.name,
                    did_you_mean(&field.name, fields.keys().map(String::as_str))
                ),
            );
        };
        self.deprecated(
            format!("{parent}.{}", field.name),
            definition.deprecation.as_ref(),
            field.position,
        );
        self.check_arguments(parent, field, definition)?;
        Ok(definition.ty.clone())
    }

    fn check_arguments(
        &mut self,
        parent: &str,
        field: &'a ast::Field<'a, String>,
        definition: &Field,
    ) -> Result<()> {
        for (index, (name, value)) in field.arguments.iter().enumerate() {
            if field.arguments[..index]
                .iter()
                .any(|(earlier, _)| earlier == name)
            {
                return error_at(
                    field.position,
                    format!(
                        "Field `{parent}.{}` is given the argument `{name}` twice",
                        field.name
                    ),
                );
            }
            let Some(argument) = definition.args.get(name) else {
                return error_at(
                    field.position,
                    format!(
                        "Field `{parent}.{}` has no argument `{name}`{}",
                        field.name,
                        did_you_mean(name, definition.args.keys().map(String::as_str))
                    ),
                );
            };
            self.deprecated(
                format!("{parent}.{}({name}:)", field.name),
                argument.deprecation.as_ref(),
                field.position,
            );
            let place = format!("argument `{name}` of `{parent}.{}`", field.name);
            self.check_value(value, argument, &place, field.position)?;
        }
        for (name, argument) in &definition.args {
            if argument.is_required() && !field.arguments.iter().any(|(given, _)| given == name) {
                return error_at(
                    field.position,
                    format!(
                        "Field `{parent}.{}` requires the argument `{name}: {}`",
                        field.name, argument.ty
                    ),
                );
            }
        }
        Ok(())
    }

    /// Checks the directives written at `location` (`FIELD`, `QUERY`…)
    /// against those of the schema. Returns whether they make the selection
    /// conditional: `@include` and `@skip`.
    fn check_directives(
        &mut self,
        directives: &'a [Directive<'a>],
        location: &str,
    ) -> Result<bool> {
        let schema = self.schema;
        let mut conditional = false;
        for (index, directive) in directives.iter().enumerate() {
            let name = directive.name.as_str();
            let Some(definition) = schema.directive(name) else {
                return error_at(
                    directive.position,
                    format!(
                        "The schema has no directive `@{name}`{}",
                        did_you_mean(name, schema.directives.keys().map(String::as_str))
                            .replacen('`', "`@", 1)
                    ),
                );
            };
            if !definition
                .locations
                .iter()
                .any(|allowed| allowed == location)
            {
                let place = location.to_lowercase().replace('_', " ");
                return error_at(
                    directive.position,
                    format!("`@{name}` cannot be written on a {place}"),
                );
            }
            if !definition.repeatable
                && directives[..index]
                    .iter()
                    .any(|earlier| earlier.name == name)
            {
                return error_at(
                    directive.position,
                    format!("`@{name}` is written twice at the same place"),
                );
            }
            for (position, (argument, value)) in directive.arguments.iter().enumerate() {
                if directive.arguments[..position]
                    .iter()
                    .any(|(earlier, _)| earlier == argument)
                {
                    return error_at(
                        directive.position,
                        format!("`@{name}` is given the argument `{argument}` twice"),
                    );
                }
                let Some(expected) = definition.args.get(argument) else {
                    return error_at(
                        directive.position,
                        format!(
                            "`@{name}` has no argument `{argument}`{}",
                            did_you_mean(argument, definition.args.keys().map(String::as_str))
                        ),
                    );
                };
                let place = format!("argument `{argument}` of `@{name}`");
                self.check_value(value, expected, &place, directive.position)?;
            }
            if let Some((missing, expected)) =
                definition.args.iter().find(|(argument, expected)| {
                    expected.is_required()
                        && !directive
                            .arguments
                            .iter()
                            .any(|(given, _)| given == *argument)
                })
            {
                return error_at(
                    directive.position,
                    format!(
                        "`@{name}` requires the argument `{missing}: {}`",
                        expected.ty
                    ),
                );
            }
            conditional |= matches!(name, "include" | "skip");
        }
        Ok(conditional)
    }

    /// Marks the variables of a value whose type is not known.
    fn mark_variables(&mut self, value: &Value<'a>, position: Pos) -> Result<()> {
        match value {
            ast::Value::Variable(name) => {
                self.variable(name, position)?;
            }
            ast::Value::List(items) => {
                for item in items {
                    self.mark_variables(item, position)?;
                }
            }
            ast::Value::Object(fields) => {
                for item in fields.values() {
                    self.mark_variables(item, position)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// The declared variable of this name, which is thereby used.
    fn variable(&mut self, name: &str, position: Pos) -> Result<&Variable> {
        let Some(variable) = self.variables.iter().find(|variable| variable.name == name) else {
            let declared = self.variables.iter().map(|variable| variable.name.as_str());
            return error_at(
                position,
                format!(
                    "Variable `${name}` is not declared{}",
                    did_you_mean(name, declared)
                ),
            );
        };
        self.used_variables.insert(name.to_string());
        Ok(variable)
    }

    /// The rule of a `@oneOf` input object: exactly one field, whose value is
    /// not `null`, nor a variable that may be.
    fn check_one_of(
        &mut self,
        named: &str,
        fields: &BTreeMap<String, InputValue>,
        given: &BTreeMap<String, Value<'a>>,
        place: &str,
        position: Pos,
    ) -> Result<()> {
        let names: Vec<String> = fields.keys().map(|key| format!("`{key}`")).collect();
        let rule = format!(
            "`{named}` is a oneOf input: give exactly one of {}, not null (in {place})",
            crate::literal::join(&names, "or")
        );
        let mut entries = given.iter();
        let (Some((name, value)), None) = (entries.next(), entries.next()) else {
            return error_at(position, rule);
        };
        match value {
            ast::Value::Null => error_at(position, format!("{rule}: `{name}: null` is not one")),
            ast::Value::Variable(variable) => {
                let ty = self.variable(variable, position)?.ty.clone();
                if ty.is_non_null() {
                    return Ok(());
                }
                error_at(
                    position,
                    format!(
                        "{rule}: `${variable}` is of type `{ty}`, which may be null; declare it `{ty}!`"
                    ),
                )
            }
            _ => Ok(()),
        }
    }

    /// Checks a literal or a variable given where `expected` is wanted.
    fn check_value(
        &mut self,
        value: &Value<'a>,
        expected: &InputValue,
        place: &str,
        position: Pos,
    ) -> Result<()> {
        let inner = expected.ty.nullable();
        // A custom scalar, `JSON` among them, defines its own literals: an object
        // or a list is a value of it, not an input object or a list of the schema.
        if let TypeRef::Named(name) = inner
            && !matches!(value, ast::Value::Variable(_) | ast::Value::Null)
            && !matches!(name.as_str(), "Int" | "Float" | "String" | "Boolean" | "ID")
            && matches!(self.schema.get(name), Some(TypeDef::Scalar))
        {
            return self.mark_variables(value, position);
        }
        match value {
            ast::Value::Variable(name) => {
                let null_default = self.null_defaults.contains(name);
                let variable = self.variable(name, position)?;
                if !variable_fits(variable, null_default, expected) {
                    return error_at(
                        position,
                        format!(
                            "Variable `${name}` of type `{}` cannot be used for {place}, which expects `{}`",
                            variable.ty, expected.ty
                        ),
                    );
                }
            }
            ast::Value::Null => {
                if expected.ty.is_non_null() {
                    return error_at(
                        position,
                        format!(
                            "`null` is not allowed for {place}, of type `{}`",
                            expected.ty
                        ),
                    );
                }
            }
            ast::Value::List(items) => {
                // GraphQL accepts a single value where a list is expected, not the opposite.
                let TypeRef::List(item_type) = inner else {
                    return error_at(
                        position,
                        format!("A list is given for {place}, of type `{}`", expected.ty),
                    );
                };
                let item = InputValue::of((**item_type).clone());
                for value in items {
                    self.check_value(value, &item, place, position)?;
                }
            }
            ast::Value::Object(given) => {
                let named = inner.name();
                let Some(TypeDef::Input { fields, one_of }) = self.schema.get(named) else {
                    return error_at(
                        position,
                        format!("An object is given for {place}, of type `{}`", expected.ty),
                    );
                };
                if *one_of {
                    self.check_one_of(named, fields, given, place, position)?;
                }
                for (name, value) in given {
                    let Some(field) = fields.get(name) else {
                        return error_at(
                            position,
                            format!(
                                "Input type `{named}` has no field `{name}`{} (in {place})",
                                did_you_mean(name, fields.keys().map(String::as_str))
                            ),
                        );
                    };
                    self.deprecated(
                        format!("{named}.{name}"),
                        field.deprecation.as_ref(),
                        position,
                    );
                    self.check_value(
                        value,
                        field,
                        &format!("field `{name}` of `{named}`"),
                        position,
                    )?;
                }
                if let Some((missing, field)) = fields
                    .iter()
                    .find(|(name, field)| field.is_required() && !given.contains_key(*name))
                {
                    return error_at(
                        position,
                        format!(
                            "Input type `{named}` requires the field `{missing}: {}` (in {place})",
                            field.ty
                        ),
                    );
                }
            }
            ast::Value::Enum(given) => match self.schema.get(inner.name()) {
                Some(TypeDef::Enum { values, deprecated })
                    if matches!(inner, TypeRef::Named(_)) =>
                {
                    if !values.contains(given) {
                        return error_at(
                            position,
                            format!(
                                "`{given}` is not a value of the enum `{}`{} (in {place})",
                                inner.name(),
                                did_you_mean(given, values.iter().map(String::as_str))
                            ),
                        );
                    }
                    let reason = deprecated.get(given).cloned();
                    self.deprecated(
                        format!("{}.{given}", inner.name()),
                        reason.as_ref(),
                        position,
                    );
                }
                // A single value where a list of enum values is expected.
                Some(TypeDef::Enum { .. }) => {
                    let item = InputValue::of(TypeRef::Named(inner.name().to_string()));
                    self.check_value(value, &item, place, position)?;
                }
                _ => {
                    return error_at(
                        position,
                        format!(
                            "The enum value `{given}` is given for {place}, of type `{}`",
                            expected.ty
                        ),
                    );
                }
            },
            ast::Value::Int(number) if inner.name() == "Int" && !fits_int(number) => {
                return error_at(
                    position,
                    format!(
                        "The integer is outside the range of `Int`, a 32-bit integer, given for {place}"
                    ),
                );
            }
            literal => {
                let given = match literal {
                    ast::Value::Int(_) => "an integer",
                    ast::Value::Float(_) => "a float",
                    ast::Value::String(_) => "a string",
                    _ => "a boolean",
                };
                let accepted = match (inner.name(), literal) {
                    ("Int", ast::Value::Int(_)) => true,
                    ("Float", ast::Value::Int(_) | ast::Value::Float(_)) => true,
                    ("String", ast::Value::String(_)) => true,
                    ("Boolean", ast::Value::Boolean(_)) => true,
                    ("ID", ast::Value::String(_) | ast::Value::Int(_)) => true,
                    ("Int" | "Float" | "String" | "Boolean" | "ID", _) => false,
                    // Custom scalars define their own literals; enums and input objects do not accept these.
                    (name, _) => matches!(self.schema.get(name), Some(TypeDef::Scalar)),
                };
                if !accepted {
                    return error_at(
                        position,
                        format!("{given} is given for {place}, of type `{}`", expected.ty),
                    );
                }
            }
        }
        Ok(())
    }
}

/// `Int` is a signed 32-bit integer in GraphQL.
fn fits_int(number: &ast::Number) -> bool {
    number
        .as_i64()
        .is_some_and(|number| i32::try_from(number).is_ok())
}

/// The rule of the GraphQL specification for using a variable at a location
/// ("All Variable Usages Are Allowed"). `null_default`: the default value of
/// the variable is `null`.
fn variable_fits(variable: &Variable, null_default: bool, location: &InputValue) -> bool {
    fn compatible(variable: &TypeRef, location: &TypeRef) -> bool {
        match (variable, location) {
            (TypeRef::NonNull(variable), TypeRef::NonNull(location)) => {
                compatible(variable, location)
            }
            (_, TypeRef::NonNull(_)) => false,
            (TypeRef::NonNull(variable), location) => compatible(variable, location),
            (TypeRef::List(variable), TypeRef::List(location)) => compatible(variable, location),
            (TypeRef::Named(variable), TypeRef::Named(location)) => variable == location,
            _ => false,
        }
    }
    if location.ty.is_non_null() && !variable.ty.is_non_null() {
        // A nullable variable is accepted when a default value other than
        // `null` fills the gap.
        return ((variable.has_default && !null_default) || location.has_default)
            && compatible(&variable.ty, location.ty.nullable());
    }
    compatible(&variable.ty, &location.ty)
}

/// ` (did you mean `x`?)` for the candidate closest to `name`, or nothing.
pub(crate) fn did_you_mean<'a>(name: &str, candidates: impl Iterator<Item = &'a str>) -> String {
    let lowered = name.to_lowercase();
    // Half of the name may differ, within three edits: `frist` → `first`, `CREATED` → `CREATED_AT`.
    let limit = (name.len() / 2).clamp(1, 3);
    candidates
        .map(|candidate| {
            (
                edit_distance(&lowered, &candidate.to_lowercase()),
                candidate,
            )
        })
        .filter(|(distance, _)| *distance <= limit)
        .min()
        .map(|(_, candidate)| format!(" (did you mean `{candidate}`?)"))
        .unwrap_or_default()
}

fn edit_distance(left: &str, right: &str) -> usize {
    let right: Vec<char> = right.chars().collect();
    let mut row: Vec<usize> = (0..=right.len()).collect();
    for (i, a) in left.chars().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, b) in right.iter().enumerate() {
            let substitution = previous + usize::from(a != *b);
            previous = row[j + 1];
            row[j + 1] = substitution.min(previous + 1).min(row[j] + 1);
        }
    }
    row[right.len()]
}
