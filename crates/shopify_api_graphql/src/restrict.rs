//! A schema cut down to what some operations use: the types they reach, and
//! of objects and interfaces, the fields they select.
//!
//! `shopify_api` checks its own operations against such a schema, a few
//! kilobytes instead of the whole Admin API.

use std::collections::{BTreeMap, BTreeSet};

use graphql_parser::query as ast;

use crate::schema::{Field, InputValue, Schema, TypeDef};
use crate::{Error, analyze};

type Document<'a> = ast::Document<'a, String>;
type SelectionSet<'a> = ast::SelectionSet<'a, String>;

impl Schema {
    /// The part of the schema `operations` use. Each operation is checked
    /// first, and is accepted by the result as by `self`, with the same shape.
    pub fn restricted_to(&self, operations: &[&str]) -> Result<Schema, Error> {
        let mut used = Used::default();
        for source in operations {
            analyze(self, source)?;
            let document: Document =
                graphql_parser::parse_query(source).expect("an operation analyze accepted parses");
            used.document(self, &document);
        }
        Ok(used.schema(self))
    }
}

/// What the operations reach, before the closure over types.
#[derive(Default)]
struct Used {
    /// Selected fields, by object or interface.
    fields: BTreeMap<String, BTreeSet<String>>,
    /// Types named by variables and type conditions.
    types: BTreeSet<String>,
}

impl Used {
    fn document<'a>(&mut self, schema: &Schema, document: &'a Document<'a>) {
        let fragments: BTreeMap<&str, &'a ast::FragmentDefinition<'a, String>> = document
            .definitions
            .iter()
            .filter_map(|definition| match definition {
                ast::Definition::Fragment(fragment) => Some((fragment.name.as_str(), fragment)),
                ast::Definition::Operation(_) => None,
            })
            .collect();
        for definition in &document.definitions {
            let ast::Definition::Operation(operation) = definition else {
                continue;
            };
            let (root, variables, set) = match operation {
                ast::OperationDefinition::SelectionSet(set) => (&schema.query, &[][..], set),
                ast::OperationDefinition::Query(query) => (
                    &schema.query,
                    &query.variable_definitions[..],
                    &query.selection_set,
                ),
                ast::OperationDefinition::Mutation(mutation) => (
                    schema.mutation.as_ref().unwrap_or(&schema.query),
                    &mutation.variable_definitions[..],
                    &mutation.selection_set,
                ),
                ast::OperationDefinition::Subscription(_) => continue,
            };
            for variable in variables {
                self.types.insert(named(&variable.var_type).to_string());
            }
            self.selection(schema, root, set, &fragments, &mut BTreeSet::new());
        }
    }

    fn selection<'a>(
        &mut self,
        schema: &Schema,
        parent: &str,
        set: &'a SelectionSet<'a>,
        fragments: &BTreeMap<&str, &'a ast::FragmentDefinition<'a, String>>,
        spread: &mut BTreeSet<&'a str>,
    ) {
        self.types.insert(parent.to_string());
        for item in &set.items {
            match item {
                ast::Selection::Field(field) => {
                    if field.name.starts_with("__") {
                        continue;
                    }
                    let fields = match schema.get(parent) {
                        Some(
                            TypeDef::Object { fields, .. } | TypeDef::Interface { fields, .. },
                        ) => fields,
                        _ => continue,
                    };
                    let Some(definition) = fields.get(&field.name) else {
                        continue;
                    };
                    self.fields
                        .entry(parent.to_string())
                        .or_default()
                        .insert(field.name.clone());
                    let inner = definition.ty.name();
                    if schema.get(inner).is_some_and(TypeDef::is_composite) {
                        self.selection(schema, inner, &field.selection_set, fragments, spread);
                    }
                }
                ast::Selection::InlineFragment(fragment) => {
                    let condition = match &fragment.type_condition {
                        Some(ast::TypeCondition::On(condition)) => condition.as_str(),
                        None => parent,
                    };
                    self.selection(
                        schema,
                        condition,
                        &fragment.selection_set,
                        fragments,
                        spread,
                    );
                }
                ast::Selection::FragmentSpread(item) => {
                    let Some(fragment) = fragments.get(item.fragment_name.as_str()) else {
                        continue;
                    };
                    if !spread.insert(fragment.name.as_str()) {
                        continue;
                    }
                    let ast::TypeCondition::On(condition) = &fragment.type_condition;
                    self.selection(
                        schema,
                        condition,
                        &fragment.selection_set,
                        fragments,
                        spread,
                    );
                    spread.remove(fragment.name.as_str());
                }
            }
        }
    }

    /// The schema of the used types, closed over the types they refer to.
    fn schema(self, schema: &Schema) -> Schema {
        let mut pending: Vec<String> = self.types.iter().cloned().collect();
        pending.push(schema.query.clone());
        pending.extend(schema.mutation.clone());
        for directive in schema.directives.values() {
            pending.extend(directive.args.values().map(|arg| arg.ty.name().to_string()));
        }
        let mut types = BTreeMap::new();
        while let Some(name) = pending.pop() {
            if types.contains_key(&name) {
                continue;
            }
            let Some(definition) = schema.get(&name) else {
                continue;
            };
            let selected = self.fields.get(&name);
            let kept = |fields: &BTreeMap<String, Field>, pending: &mut Vec<String>| {
                let fields: BTreeMap<String, Field> = fields
                    .iter()
                    .filter(|(field, _)| selected.is_some_and(|selected| selected.contains(*field)))
                    .map(|(field, definition)| (field.clone(), definition.clone()))
                    .collect();
                for field in fields.values() {
                    pending.push(field.ty.name().to_string());
                    pending.extend(field.args.values().map(|arg| arg.ty.name().to_string()));
                }
                fields
            };
            let restricted = match definition {
                TypeDef::Object { fields, interfaces } => TypeDef::Object {
                    fields: kept(fields, &mut pending),
                    interfaces: interfaces.clone(),
                },
                TypeDef::Interface {
                    fields,
                    possible_types,
                } => TypeDef::Interface {
                    fields: kept(fields, &mut pending),
                    possible_types: possible_types.clone(),
                },
                TypeDef::Input { fields, .. } => {
                    pending.extend(
                        fields
                            .values()
                            .map(|field: &InputValue| field.ty.name().to_string()),
                    );
                    definition.clone()
                }
                TypeDef::Scalar | TypeDef::Enum { .. } | TypeDef::Union { .. } => {
                    definition.clone()
                }
            };
            types.insert(name, restricted);
        }
        // Interfaces only among the types kept. The possible types of an
        // interface or a union all stay, kept or not: they tell whether a
        // fragment always applies, and so whether its fields are optional.
        // Left out, `... on Product` in a `Node` would always apply.
        let names: BTreeSet<String> = types.keys().cloned().collect();
        for definition in types.values_mut() {
            if let TypeDef::Object { interfaces, .. } = definition {
                interfaces.retain(|name| names.contains(name));
            }
        }
        Schema {
            api_version: schema.api_version.clone(),
            query: schema.query.clone(),
            mutation: schema.mutation.clone(),
            types,
            directives: schema.directives.clone(),
        }
    }
}

/// The type a variable type names, without lists and non-null.
fn named<'a>(ty: &'a ast::Type<'a, String>) -> &'a str {
    match ty {
        ast::Type::NamedType(name) => name,
        ast::Type::ListType(inner) | ast::Type::NonNullType(inner) => named(inner),
    }
}
