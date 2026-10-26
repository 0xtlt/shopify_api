//! Bulk operations: the rules checked on top of `analyze`, the text a bulk
//! query sends and where its lines go.

use std::sync::OnceLock;

use shopify_api_graphql::bulk::{
    Argument, BulkQuery, Connection, Part, ValuePart, analyze_bulk_query, check_bulk_mutation,
    query_items,
};
use shopify_api_graphql::{FieldBase, Schema, analyze};

/// A small schema: connections of `Thing`s, an input object that holds an
/// enum and itself, a `JSON` argument.
const SCHEMA: &str = r#"{"format":2,"apiVersion":"2026-10","query":"QueryRoot","mutation":"Mutation","types":{
"QueryRoot":{"o":{"f":{
  "things":["ThingConnection!",{"filter":"Filter","tags":"[Color!]","data":"JSON","first":"Int","q":"String","sort":"Color"}],
  "thing":["Thing",{"id":"ID!"}]
}}},
"Mutation":{"o":{"f":{"make":["MakePayload",{"id":"ID!","filter":"Filter"}]}}},
"MakePayload":{"o":{"f":{"a":"ThingConnection!","b":"ThingConnection!","thing":"Thing"}}},
"Node":{"n":{"f":{"id":"ID!"},"t":["Thing"]}},
"Thing":{"o":{"f":{"id":"ID!","name":"String","parts":["ThingConnection!",{"q":"String"}],"tags":"[Color!]"},"t":["Node"]}},
"ThingConnection":{"o":{"f":{"edges":"[ThingEdge!]!","nodes":"[Thing!]!","pageInfo":"PageInfo!"}}},
"ThingEdge":{"o":{"f":{"node":"Thing!","cursor":"String!"}}},
"PageInfo":{"o":{"f":{"hasNextPage":"Boolean!"}}},
"Filter":{"i":{"color":"Color","nested":"Filter","name":"String","colors":"[Color!]"}},
"Color":{"e":["RED","BLUE"]},
"ID":"s","String":"s","Boolean":"s","Int":"s","Float":"s","JSON":"s"
}}"#;

fn schema() -> &'static Schema {
    static SCHEMA_ONCE: OnceLock<Schema> = OnceLock::new();
    SCHEMA_ONCE.get_or_init(|| Schema::from_json(SCHEMA).unwrap())
}

fn bulk(source: &str) -> BulkQuery {
    let operation = analyze(schema(), source).unwrap_or_else(|error| panic!("{error}"));
    analyze_bulk_query(schema(), source, &operation).unwrap_or_else(|error| panic!("{error}"))
}

fn refusal(source: &str) -> String {
    let operation = analyze(schema(), source).unwrap_or_else(|error| panic!("{error}"));
    let refused = match operation.kind {
        shopify_api_graphql::OperationKind::Query => {
            analyze_bulk_query(schema(), source, &operation).err()
        }
        shopify_api_graphql::OperationKind::Mutation => {
            check_bulk_mutation(schema(), source, &operation).err()
        }
    };
    refused.expect("refused").to_string()
}

fn text(parts: &[Part]) -> String {
    parts
        .iter()
        .map(|part| match part {
            Part::Text(text) => text.clone(),
            _ => "(…)".to_string(),
        })
        .collect()
}

#[test]
fn the_text_keeps_constants_and_sets_variables_apart() {
    let query = bulk(
        r#"query Things($filter: Filter, $q: String = "x \"y\"", $named: Boolean!) {
            things(filter: $filter, q: $q, sort: RED) { edges { node { id name @include(if: $named) } } }
        }"#,
    );
    assert_eq!(
        text(&query.parts),
        "query Things { things(…) { edges { node { id name @include(…) } } } }"
    );
    let Part::Arguments(arguments) = &query.parts[1] else {
        panic!("arguments");
    };
    assert_eq!(
        named(arguments),
        [
            ("filter", &ValuePart::Variable("filter".into())),
            ("q", &ValuePart::Variable("q".into())),
            ("sort", &ValuePart::Text("RED".into())),
        ]
    );
    assert_eq!(query.defaults["q"], r#""x \"y\"""#);
}

fn named(arguments: &[Argument]) -> Vec<(&str, &ValuePart)> {
    arguments
        .iter()
        .map(|argument| (argument.name.as_str(), &argument.value))
        .collect()
}

#[test]
fn constants_are_written_back_as_graphql() {
    let query = bulk(
        r#"{ things(filter: {name: """block "quoted"
text""", colors: [RED, BLUE], nested: null}, q: "$q stays", sort: null) { edges { node { id } } } }"#,
    );
    assert_eq!(
        text(&query.parts),
        r#"{ things(filter: {colors: [RED, BLUE], name: "block \"quoted\"\ntext", nested: null}, q: "$q stays", sort: null) { edges { node { id } } } }"#
    );
}

#[test]
fn variables_deep_in_values_are_set_apart_with_their_place() {
    let query = bulk(
        "query($c: Color!, $n: String) { things(filter: {colors: [RED, $c], nested: {name: $n}}) { edges { node { id } } } }",
    );
    let Part::Arguments(arguments) = &query.parts[1] else {
        panic!("arguments");
    };
    let ValuePart::Object(fields) = &arguments[0].value else {
        panic!("an object");
    };
    assert_eq!(fields[0].name, "colors");
    assert_eq!(
        fields[0].value,
        ValuePart::List(vec![
            ValuePart::Text("RED".into()),
            ValuePart::Variable("c".into())
        ])
    );
    assert_eq!(fields[1].name, "nested");
    assert!(
        matches!(&fields[1].value, ValuePart::Object(nested) if nested[0].value == ValuePart::Variable("n".into()))
    );
}

#[test]
fn connections_become_lists_of_their_nodes() {
    let query = bulk(
        "{ things { edges { node { id name parts(q: \"x\") { edges { node { id parts { edges { node { name } } } } } } } } } }",
    );
    assert_eq!(shape(&query.connections), "parts: Thing [parts: Thing []]");
    let parts = query
        .item
        .fields
        .iter()
        .find(|field| field.key == "parts")
        .unwrap();
    assert!(parts.connection);
    assert_eq!(parts.ty.to_string(), "[Thing!]!");
    let FieldBase::Object(nodes) = &parts.base else {
        panic!("an object");
    };
    let nested = nodes
        .fields
        .iter()
        .find(|field| field.key == "parts")
        .unwrap();
    assert!(nested.connection);
    // One connection per parent: no `__typename` is needed.
    assert!(!text(&query.parts).contains("__typename"));
}

/// `key: Types [nested]` of each connection.
fn shape(connections: &[Connection]) -> String {
    connections
        .iter()
        .map(|connection| {
            format!(
                "{}: {} [{}]",
                connection.key,
                connection.types.join(" "),
                shape(&connection.connections)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[test]
fn siblings_that_may_return_one_type_are_refused() {
    assert_eq!(
        refusal(
            "{ things { edges { node { id a: parts { edges { node { id } } } b: parts { edges { node { id } } } } } } }"
        ),
        "`a` and `b`, connections of each node of `things`, may both return `Thing`: their lines in the results could not be told apart. Query one of them in another bulk query (line 1, column 65)"
    );
}

#[test]
fn json_variables_are_refused() {
    assert_eq!(
        refusal("query($d: JSON) { things(data: $d) { edges { node { id } } } }"),
        "Variable `$d` may hold a `JSON` value, which a bulk query cannot take: Shopify takes no variables for a bulk query, and this crate writes them into its text as GraphQL literals (line 1, column 7)"
    );
}

#[test]
fn the_rules_of_shopify_are_refusals() {
    for (source, message) in [
        (
            "{ thing(id: \"1\") { id parts { edges { node { id } } } } }",
            "The top-level field of a bulk query is a connection: `thing` is `Thing`. Run it as an ordinary query, or select a connection such as `products` (line 1, column 3)",
        ),
        (
            "{ things { edges { cursor node { id } } } }",
            "A connection of a bulk query selects `edges { node { … } }` and nothing else, as Shopify documents it (`things`): `cursor` is refused beside `node` (line 1, column 20)",
        ),
        (
            "{ things { edges { n: node { id } } } }",
            "A connection of a bulk query selects `edges { node { … } }` and nothing else, as Shopify documents it (`things`): write `node`, without alias, arguments or directives (line 1, column 20)",
        ),
        (
            "{ things { edges { node { id parts @include(if: true) { edges { node { id } } } } } } }",
            "A connection of a bulk query cannot have directives such as `@include`: Shopify writes its nodes on lines of their own (line 1, column 36)",
        ),
        (
            "{ things { edges { node { name parts { edges { node { id } } } } } } }",
            "The nodes of `things` have the nested connection `parts`: select their `id`, without an alias or a directive, in this `node { … }`. Shopify refuses the query otherwise, and places each nested line under its parent by that `id` (line 1, column 20)",
        ),
        (
            "{ things { edges { node { id @include(if: true) parts { edges { node { id } } } } } } }",
            "The nodes of `things` have the nested connection `parts`: select their `id`, without an alias or a directive, in this `node { … }`. Shopify refuses the query otherwise, and places each nested line under its parent by that `id` (line 1, column 20)",
        ),
        (
            "{ things { edges { node { id __typename: name } } } }",
            "`__typename` cannot be an alias in a bulk query: the lines of its results hold `__parentId` and `__typename` (line 1, column 30)",
        ),
        (
            "mutation { make(id: \"1\") { a { edges { node { id } } } b { edges { node { id } } } } }",
            "The mutation of a bulk operation selects one connection at most (`a`), Shopify's limit: `b` is a second one (line 1, column 56)",
        ),
        (
            "mutation { ...M } fragment M on Mutation { make(id: \"1\") { thing { id } } }",
            "A bulk mutation selects one top-level field, not a fragment (line 1, column 15)",
        ),
    ] {
        assert_eq!(refusal(source), message, "{source}");
    }
    // One connection in a mutation, at any depth.
    let source = "mutation { make(id: \"1\") { a { edges { node { id } } } thing { id } } }";
    let operation = analyze(schema(), source).unwrap();
    check_bulk_mutation(schema(), source, &operation).unwrap();
}

#[test]
fn the_enums_of_input_objects_are_described_for_the_variables() {
    let source = "query($filter: Filter, $tags: [Color!], $q: String) { things(filter: $filter, tags: $tags, q: $q) { edges { node { id } } } }";
    let operation = analyze(schema(), source).unwrap();
    let query = analyze_bulk_query(schema(), source, &operation).unwrap();
    let code = query_items(schema(), &operation, &query, &quote::quote!(::shopify_api)).to_string();
    // `Filter` holds an enum, a list of them and itself; `name` holds none.
    assert!(
        code.contains(
            "static __SHOPIFY_API_BULK_INPUT_Filter : [(& str , :: shopify_api :: __private :: bulk :: BulkType) ; 3usize] = [(\"color\" , :: shopify_api :: __private :: bulk :: BulkType :: Enum) , (\"colors\" , :: shopify_api :: __private :: bulk :: BulkType :: List (& :: shopify_api :: __private :: bulk :: BulkType :: Enum)) , (\"nested\" , :: shopify_api :: __private :: bulk :: BulkType :: Input (& __SHOPIFY_API_BULK_INPUT_Filter))]"
        ),
        "{code}"
    );
    assert!(code.contains("name : \"tags\" , ty : :: shopify_api :: __private :: bulk :: BulkType :: List (& :: shopify_api :: __private :: bulk :: BulkType :: Enum)"), "{code}");
    assert!(
        code.contains(
            "name : \"q\" , ty : :: shopify_api :: __private :: bulk :: BulkType :: Plain"
        ),
        "{code}"
    );
}
