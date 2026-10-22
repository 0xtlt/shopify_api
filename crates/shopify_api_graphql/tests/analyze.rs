//! Operations checked against the Shopify Admin schema 2026-10: what is
//! refused, with which message, and the shape of what is accepted.

use std::sync::OnceLock;

use shopify_api_graphql::{
    FieldBase, Operation, OperationKind, Scalar, Schema, Selection, VariableBase, analyze,
};

/// The Admin schema 2026-10 the tests of the workspace use.
fn admin() -> &'static Schema {
    static ADMIN: OnceLock<Schema> = OnceLock::new();
    ADMIN.get_or_init(|| {
        Schema::from_json(include_str!("../../shopify_api/graphql/admin.schema.json")).unwrap()
    })
}

fn check(source: &str) -> Operation {
    analyze(admin(), source).unwrap_or_else(|error| panic!("{error}\n{source}"))
}

fn refusal(source: &str) -> String {
    match analyze(admin(), source) {
        Ok(_) => panic!("accepted: {source}"),
        Err(error) => error.to_string(),
    }
}

/// `key: Type` of each field, `?` marking those that may be absent.
fn fields(selection: &Selection) -> Vec<String> {
    selection
        .fields
        .iter()
        .map(|field| {
            format!(
                "{}{}: {}",
                field.key,
                if field.conditional { "?" } else { "" },
                field.ty
            )
        })
        .collect()
}

fn child<'a>(selection: &'a Selection, key: &str) -> &'a Selection {
    match selection
        .fields
        .iter()
        .find(|field| field.key == key)
        .map(|field| &field.base)
    {
        Some(FieldBase::Object(selection)) => selection,
        _ => panic!("`{key}` is not a selection of `{}`", selection.type_name),
    }
}

// ─── Accepted ────────────────────────────────────────────────────────

#[test]
fn describes_variables_and_nested_selections() {
    let operation = check(
        "query Recent($first: Int!, $query: String, $sortKey: OrderSortKeys = CREATED_AT) {
           orders(first: $first, query: $query, sortKey: $sortKey) { nodes { id name note } }
         }",
    );

    assert_eq!(operation.kind, OperationKind::Query);
    assert_eq!(operation.name.as_deref(), Some("Recent"));
    let variables: Vec<(&str, String, bool, VariableBase)> = operation
        .variables
        .iter()
        .map(|variable| {
            (
                variable.name.as_str(),
                variable.ty.to_string(),
                variable.is_required(),
                variable.base,
            )
        })
        .collect();
    assert_eq!(
        variables,
        [
            (
                "first",
                "Int!".to_string(),
                true,
                VariableBase::Scalar(Scalar::Int)
            ),
            (
                "query",
                "String".to_string(),
                false,
                VariableBase::Scalar(Scalar::String)
            ),
            (
                "sortKey",
                "OrderSortKeys".to_string(),
                false,
                VariableBase::Enum
            ),
        ]
    );

    assert_eq!(operation.data.type_name, "QueryRoot");
    assert_eq!(fields(&operation.data), ["orders: OrderConnection!"]);
    let orders = child(&operation.data, "orders");
    assert_eq!(fields(orders), ["nodes: [Order!]!"]);
    let nodes = child(orders, "nodes");
    assert_eq!(nodes.type_name, "Order");
    assert_eq!(fields(nodes), ["id: ID!", "name: String!", "note: String"]);
}

#[test]
fn accepts_the_shorthand_query_and_mutations() {
    let operation = check("{ shop { name } }");
    assert_eq!(
        (operation.kind, operation.name),
        (OperationKind::Query, None)
    );

    let operation = check(
        "mutation($metafields: [MetafieldsSetInput!]!) {
           metafieldsSet(metafields: $metafields) { userErrors { message } }
         }",
    );
    assert_eq!(operation.kind, OperationKind::Mutation);
    assert_eq!(operation.data.type_name, "Mutation");
    assert_eq!(operation.variables[0].base, VariableBase::Input);
    assert_eq!(
        fields(&operation.data),
        ["metafieldsSet: MetafieldsSetPayload"]
    );
}

#[test]
fn maps_scalars_to_their_json_form() {
    let operation = check(
        "{ shop { name } order(id: \"gid://shopify/Order/1\") {
             createdAt
             subtotalLineItemsQuantity
             totalWeight
             confirmed
             metafield(key: \"k\") { jsonValue }
             totalPriceSet { shopMoney { amount } }
           } }",
    );
    let order = child(&operation.data, "order");
    let scalar = |selection: &Selection, key: &str| match selection
        .fields
        .iter()
        .find(|field| field.key == key)
        .map(|field| &field.base)
    {
        Some(FieldBase::Scalar(scalar)) => *scalar,
        _ => panic!("`{key}` is not a scalar"),
    };
    assert_eq!(scalar(order, "createdAt"), Scalar::String);
    assert_eq!(scalar(order, "subtotalLineItemsQuantity"), Scalar::Int);
    // `UnsignedInt64` travels as a string.
    assert_eq!(scalar(order, "totalWeight"), Scalar::String);
    assert_eq!(scalar(order, "confirmed"), Scalar::Boolean);
    assert_eq!(scalar(child(order, "metafield"), "jsonValue"), Scalar::Json);
    let money = child(child(order, "totalPriceSet"), "shopMoney");
    assert_eq!(scalar(money, "amount"), Scalar::String);
}

#[test]
fn merges_selections_of_the_same_field_and_marks_conditional_ones() {
    let operation = check(
        "query($withTags: Boolean!, $ids: [ID!]!) {
           nodes(ids: $ids) {
             id
             __typename
             ... on Node { id }
             ... on Product { title handle ...ProductBits }
             ... on Order { name tags @include(if: $withTags) }
             ... on HasMetafields { metafield(key: \"k\") { value } }
           }
         }
         fragment ProductBits on Product { title vendor featuredMedia { id } }",
    );
    let nodes = child(&operation.data, "nodes");
    assert_eq!(nodes.type_name, "Node");
    assert_eq!(
        fields(nodes),
        [
            "id: ID!",
            "__typename: String!",
            "title?: String!",
            "handle?: String!",
            "vendor?: String!",
            "featuredMedia?: Media",
            "name?: String!",
            "tags?: [String!]!",
            "metafield?: Metafield",
        ]
    );

    // On the object itself, a fragment on one of its interfaces always applies.
    let operation = check(
        "{ shop { ... on Node { id } ... on Shop { name } ...Meta } }
         fragment Meta on HasMetafields { metafield(key: \"k\") { value } }",
    );
    assert_eq!(
        fields(child(&operation.data, "shop")),
        ["id: ID!", "name: String!", "metafield: Metafield"]
    );

    // The same field selected twice is one field with both selections.
    let operation =
        check("{ shop { plan { publicDisplayName } plan { partnerDevelopment } name name } }");
    let shop = child(&operation.data, "shop");
    assert_eq!(fields(shop), ["plan: ShopPlan!", "name: String!"]);
    assert_eq!(
        fields(child(shop, "plan")),
        ["publicDisplayName: String!", "partnerDevelopment: Boolean!"]
    );
}

#[test]
fn accepts_literals_of_every_kind() {
    check(
        "mutation {
           metafieldsSet(metafields: [{ ownerId: \"gid://shopify/Product/1\", key: \"k\", value: \"v\", namespace: null }]) {
             userErrors { message }
           }
         }",
    );
    check(
        "{ orders(first: 5, reverse: true, sortKey: UPDATED_AT, query: \"status:open\") { nodes { id } } }",
    );
    // An integer where an ID is expected, and a variable with a default for a non-null argument.
    check("query($id: ID = 1) { product(id: $id) { id } node(id: 2) { id } }");
    check(r#"query($id: ID = "gid://shopify/Product/1") { product(id: $id) { id } }"#);
    check(r#"query($ids: [ID!] = ["gid://shopify/Product/1"]) { nodes(ids: $ids) { id } }"#);
    // A single value where a list is expected.
    check("{ nodes(ids: \"gid://shopify/Product/1\") { id } }");
}

// ─── Refused ─────────────────────────────────────────────────────────

/// Input Object Field Uniqueness: the parser would keep one, Shopify gets both.
#[test]
fn refuses_an_input_field_given_twice() {
    assert_eq!(
        refusal(
            r#"mutation { productUpdate(product: {id: "1", title: "old", title: "new"}) { product { id } } }"#
        ),
        "The input field `title` is given twice in the same input object (line 1, column 59)"
    );
    check(
        r#"mutation { productUpdate(product: {id: "1", title: "new"}) { product { id title } } }"#,
    );
}

/// Only a default other than `null` lets a nullable variable fill a non-null place.
#[test]
fn refuses_a_null_default_for_a_non_null_place() {
    assert_eq!(
        refusal("query($id: ID = null) { product(id: $id) { id } }"),
        "Variable `$id` of type `ID` cannot be used for argument `id` of `QueryRoot.product`, which expects `ID!` (line 1, column 25)"
    );
    assert!(
        refusal("query($ids: [ID!] = null) { nodes(ids: $ids) { id } }")
            .starts_with("Variable `$ids` of type `[ID!]` cannot be used"),
    );
    // Where `null` is allowed, a `null` default is fine.
    check("query($after: String = null) { products(first: 1, after: $after) { nodes { id } } }");
}

#[test]
fn refuses_unknown_fields_and_arguments_with_a_suggestion() {
    assert_eq!(
        refusal("{ orders(first: 1) { nodes { id nam } } }"),
        "Type `Order` has no field `nam` (did you mean `name`?) (line 1, column 33)"
    );
    assert_eq!(
        refusal("{ orders(frist: 1) { nodes { id } } }"),
        "Field `QueryRoot.orders` has no argument `frist` (did you mean `first`?) (line 1, column 3)"
    );
    assert_eq!(
        refusal("{ shop { zzzzzz } }"),
        "Type `Shop` has no field `zzzzzz` (line 1, column 10)"
    );
    assert_eq!(
        refusal("{ shop { __typename(x: 1) } }"),
        "Field `__typename` has no argument `x` (line 1, column 10)"
    );
}

#[test]
fn refuses_missing_arguments_and_selections() {
    assert_eq!(
        refusal("{ product { id } }"),
        "Field `QueryRoot.product` requires the argument `id: ID!` (line 1, column 3)"
    );
    assert_eq!(
        refusal("{ shop }"),
        "Field `shop` has the type `Shop!`: select the subfields you need, `shop { … }` (line 1, column 3)"
    );
    assert_eq!(
        refusal("{ shop { name { length } } }"),
        "Field `name` has the type `String!`, which has no subfields (line 1, column 10)"
    );
}

#[test]
fn refuses_variables_that_do_not_fit() {
    assert_eq!(
        refusal("query($first: String!) { orders(first: $first) { nodes { id } } }"),
        "Variable `$first` of type `String!` cannot be used for argument `first` of `QueryRoot.orders`, which expects `Int` (line 1, column 26)"
    );
    assert_eq!(
        refusal("query($id: ID) { product(id: $id) { id } }"),
        "Variable `$id` of type `ID` cannot be used for argument `id` of `QueryRoot.product`, which expects `ID!` (line 1, column 18)"
    );
    assert_eq!(
        refusal("query($ids: [ID]!) { nodes(ids: $ids) { id } }"),
        "Variable `$ids` of type `[ID]!` cannot be used for argument `ids` of `QueryRoot.nodes`, which expects `[ID!]!` (line 1, column 22)"
    );
    assert_eq!(
        refusal("{ orders(first: $first) { nodes { id } } }"),
        "Variable `$first` is not declared (line 1, column 3)"
    );
    assert_eq!(
        refusal("query($first: Int!, $frist: Int) { orders(first: $firs) { nodes { id } } }"),
        "Variable `$firs` is not declared (did you mean `first`?) (line 1, column 36)"
    );
    assert_eq!(
        refusal("query($first: Int!, $after: String) { orders(first: $first) { nodes { id } } }"),
        "Variable `$after` is declared but never used"
    );
    assert_eq!(
        refusal("query($first: Int!, $first: Int!) { orders(first: $first) { nodes { id } } }"),
        "Variable `$first` is declared twice (line 1, column 21)"
    );
    assert_eq!(
        refusal("query($sort: OrderSortKey) { orders(first: 1, sortKey: $sort) { nodes { id } } }"),
        "Variable `$sort` has the unknown type `OrderSortKey` (did you mean `OrderSortKeys`?) (line 1, column 7)"
    );
    assert_eq!(
        refusal("query($order: Order) { shop { name } }"),
        "Variable `$order` cannot have the type `Order`: `Order` is an object, not an input type (line 1, column 7)"
    );
    assert_eq!(
        refusal("query($flag: String!) { shop { name @include(if: $flag) } }"),
        "Variable `$flag` of type `String!` cannot be used for argument `if` of `@include`, which expects `Boolean!` (line 1, column 37)"
    );
}

#[test]
fn refuses_literals_that_do_not_fit() {
    assert_eq!(
        refusal("{ orders(first: \"1\") { nodes { id } } }"),
        "a string is given for argument `first` of `QueryRoot.orders`, of type `Int` (line 1, column 3)"
    );
    assert_eq!(
        refusal("{ orders(first: 1, reverse: 1) { nodes { id } } }"),
        "an integer is given for argument `reverse` of `QueryRoot.orders`, of type `Boolean` (line 1, column 3)"
    );
    assert_eq!(
        refusal("{ orders(first: 1, sortKey: CREATED) { nodes { id } } }"),
        "`CREATED` is not a value of the enum `OrderSortKeys` (did you mean `CREATED_AT`?) (in argument `sortKey` of `QueryRoot.orders`) (line 1, column 3)"
    );
    assert_eq!(
        refusal("{ orders(first: 1, sortKey: \"CREATED_AT\") { nodes { id } } }"),
        "a string is given for argument `sortKey` of `QueryRoot.orders`, of type `OrderSortKeys` (line 1, column 3)"
    );
    assert_eq!(
        refusal("{ orders(first: FIVE) { nodes { id } } }"),
        "The enum value `FIVE` is given for argument `first` of `QueryRoot.orders`, of type `Int` (line 1, column 3)"
    );
    assert_eq!(
        refusal("{ product(id: null) { id } }"),
        "`null` is not allowed for argument `id` of `QueryRoot.product`, of type `ID!` (line 1, column 3)"
    );
    assert_eq!(
        refusal("{ product(id: [\"a\"]) { id } }"),
        "A list is given for argument `id` of `QueryRoot.product`, of type `ID!` (line 1, column 3)"
    );
    assert_eq!(
        refusal("{ product(id: { value: 1 }) { id } }"),
        "An object is given for argument `id` of `QueryRoot.product`, of type `ID!` (line 1, column 3)"
    );
}

#[test]
fn int_literals_are_32_bit_integers() {
    check("{ orders(first: 2147483647) { nodes { id } } }");
    check("query($first: Int = -2147483648) { orders(first: $first) { nodes { id } } }");
    assert_eq!(
        refusal("{ orders(first: 2147483648) { nodes { id } } }"),
        "The integer is outside the range of `Int`, a 32-bit integer, given for argument `first` of `QueryRoot.orders` (line 1, column 3)"
    );
    assert_eq!(
        refusal("{ orders(first: -2147483649) { nodes { id } } }"),
        "The integer is outside the range of `Int`, a 32-bit integer, given for argument `first` of `QueryRoot.orders` (line 1, column 3)"
    );
    // A float, an ID or a custom scalar is not bounded by `Int`.
    check("{ orders(first: 1) { nodes { id } } product(id: 99999999999) { id } }");
}

#[test]
fn default_values_fit_the_type_of_their_variable() {
    check(
        "query($first: Int! = 10, $sort: OrderSortKeys = CREATED_AT, $ids: [ID!] = [\"1\"]) { orders(first: $first, sortKey: $sort) { nodes { id } } nodes(ids: $ids) { id } }",
    );
    assert_eq!(
        refusal("query($first: Int = \"ten\") { orders(first: $first) { nodes { id } } }"),
        "a string is given for the default value of `$first`, of type `Int` (line 1, column 7)"
    );
    assert_eq!(
        refusal("query($first: Int! = null) { orders(first: $first) { nodes { id } } }"),
        "`null` is not allowed for the default value of `$first`, of type `Int!` (line 1, column 7)"
    );
    assert_eq!(
        refusal(
            "query($sort: OrderSortKeys = CREATED) { orders(first: 1, sortKey: $sort) { nodes { id } } }"
        ),
        "`CREATED` is not a value of the enum `OrderSortKeys` (did you mean `CREATED_AT`?) (in the default value of `$sort`) (line 1, column 7)"
    );
    assert_eq!(
        refusal("query($first: Int = 3000000000) { orders(first: $first) { nodes { id } } }"),
        "The integer is outside the range of `Int`, a 32-bit integer, given for the default value of `$first` (line 1, column 7)"
    );
}

#[test]
fn json_and_custom_scalars_accept_any_literal() {
    // `JSON` takes an object, a list or a scalar, in a default value and in an argument.
    let operation = check(
        "mutation($payload: JSON = {Amount: \"30\", customer_id: 12345, tags: [\"a\", {nested: true}]}) {
           flowTriggerReceive(handle: \"h\", payload: $payload) { userErrors { message } }
         }",
    );
    assert_eq!(operation.variables.len(), 1);
    check(
        "mutation($items: JSON = [1, \"two\", null]) { flowTriggerReceive(handle: \"h\", payload: $items) { userErrors { message } } }",
    );
    check(
        "mutation { flowTriggerReceive(handle: \"h\", payload: {Amount: \"30\", list: [1, 2], none: null}) { userErrors { message } } }",
    );
    check(
        "mutation($id: String) { flowTriggerReceive(handle: \"h\", payload: {customer: $id}) { userErrors { message } } }",
    );
    check(
        "mutation { flowTriggerReceive(handle: \"h\", payload: [1, 2]) { userErrors { message } } }",
    );
    // A variable inside the literal is declared like any other.
    assert_eq!(
        refusal(
            "mutation { flowTriggerReceive(handle: \"h\", payload: {customer: $id}) { userErrors { message } } }"
        ),
        "Variable `$id` is not declared (line 1, column 12)"
    );
    // An object stays refused for a built-in scalar.
    assert_eq!(
        refusal(
            "mutation { flowTriggerReceive(handle: {a: 1}, payload: {}) { userErrors { message } } }"
        ),
        "An object is given for argument `handle` of `Mutation.flowTriggerReceive`, of type `String` (line 1, column 12)"
    );
}

#[test]
fn checks_input_object_literals() {
    assert_eq!(
        refusal(
            "mutation { metafieldsSet(metafields: [{ ownerId: \"x\", key: \"k\", valeu: \"v\" }]) { userErrors { message } } }"
        ),
        "Input type `MetafieldsSetInput` has no field `valeu` (did you mean `value`?) (in argument `metafields` of `Mutation.metafieldsSet`) (line 1, column 12)"
    );
    assert_eq!(
        refusal(
            "mutation { metafieldsSet(metafields: [{ ownerId: \"x\", key: \"k\" }]) { userErrors { message } } }"
        ),
        "Input type `MetafieldsSetInput` requires the field `value: String!` (in argument `metafields` of `Mutation.metafieldsSet`) (line 1, column 12)"
    );
    assert_eq!(
        refusal(
            "mutation { metafieldsSet(metafields: [{ ownerId: \"x\", key: 1, value: \"v\" }]) { userErrors { message } } }"
        ),
        "an integer is given for field `key` of `MetafieldsSetInput`, of type `String!` (line 1, column 12)"
    );
}

#[test]
fn refuses_fragments_that_cannot_apply() {
    assert_eq!(
        refusal("{ shop { ... on Order { id } } }"),
        "A fragment on `Order` never applies to a `Shop` (line 1, column 14)"
    );
    assert_eq!(
        refusal("{ node(id: 1) { ... on Prodct { title } } }"),
        "A fragment is on the unknown type `Prodct` (did you mean `Product`?) (line 1, column 21)"
    );
    assert_eq!(
        refusal("{ shop { ... on ID { length } } }"),
        "A fragment cannot be on `ID`, which is a scalar (line 1, column 14)"
    );
    assert_eq!(
        refusal("{ shop { ...Missing } }"),
        "Fragment `Missing` is not defined; a `graphql!` document contains the fragments it uses (line 1, column 13)"
    );
    assert_eq!(
        refusal("{ shop { name } } fragment Unused on Shop { id }"),
        "Fragment `Unused` is defined but never used (line 1, column 19)"
    );
    assert_eq!(
        refusal("{ shop { ...A } } fragment A on Shop { name ...B } fragment B on Shop { ...A }"),
        "Fragment `A` includes itself (line 1, column 76)"
    );
    assert_eq!(
        refusal("{ shop { ...A } } fragment A on Shop { id } fragment A on Shop { name }"),
        "Fragment `A` is defined twice (line 1, column 45)"
    );
}

#[test]
fn refuses_ambiguous_keys_and_unions_without_fragments() {
    assert_eq!(
        refusal("{ shop { name name: email } }"),
        "`name` is selected twice with different fields or types (`name: String!` and `email: String!`); give one of them an alias (line 1, column 15)"
    );
    assert_eq!(
        refusal(
            "{ nodes(ids: []) { ... on Product { value: title } ... on Order { value: subtotalLineItemsQuantity } } }"
        ),
        "`value` is selected twice with different fields or types (`title: String!` and `subtotalLineItemsQuantity: Int!`); give one of them an alias (line 1, column 67)"
    );
    let message = refusal("{ order(id: 1) { metafield(key: \"k\") { reference { id } } } }");
    assert!(
        message.starts_with(
            "`MetafieldReference` is a union: select `id` inside a fragment such as `... on "
        ),
        "{message}"
    );
}

#[test]
fn refuses_documents_that_are_not_one_operation() {
    assert_eq!(
        refusal("{ shop { name } } query Other { shop { id } }"),
        "The document contains several operations; `graphql!` takes one (line 1, column 19)"
    );
    assert_eq!(
        refusal("fragment A on Shop { id }"),
        "The document contains no query or mutation"
    );
    assert_eq!(
        refusal("subscription { shop { name } }"),
        "Subscriptions are not supported (line 1, column 1)"
    );
    let message = refusal("{ shop { name }");
    assert!(message.starts_with("Invalid GraphQL: "), "{message}");
}

// ─── Deprecations ────────────────────────────────────────────────────

/// `coordinate @ line:column` of each deprecation.
fn deprecations(source: &str) -> Vec<String> {
    check(source)
        .deprecations
        .iter()
        .map(|deprecation| {
            let (line, column) = deprecation.position;
            format!("{} @ {line}:{column}", deprecation.coordinate)
        })
        .collect()
}

#[test]
fn reports_the_deprecated_fields_arguments_and_values_used() {
    assert!(deprecations("{ shop { name } }").is_empty());

    let operation = check("{ product(id: 1) { bodyHtml descriptionHtml } }");
    assert_eq!(operation.deprecations.len(), 1);
    let deprecation = &operation.deprecations[0];
    assert_eq!(deprecation.coordinate, "Product.bodyHtml");
    assert_eq!(deprecation.reason, "Use `descriptionHtml` instead.");
    assert_eq!(deprecation.position, (1, 20));

    // An argument, an enum value, an input field, and fields inside fragments.
    assert_eq!(
        deprecations(
            "query {
               collection(id: 1) { image(maxWidth: 10) { url } }
               shop { currencyCode ...Legacy }
             }
             fragment Legacy on Shop { analyticsToken }"
        ),
        [
            "Collection.image(maxWidth:) @ 2:36",
            "Shop.analyticsToken @ 5:40",
        ]
    );
    assert_eq!(
        deprecations(
            "mutation { productVariantsBulkCreate(productId: 1, variants: [{ price: 1 }], strategy: DEFAULT) { userErrors { message } } }"
        ),
        Vec::<String>::new()
    );
    let operation = check(
        "mutation($id: ID!) {
           collectionUpdate(input: { id: $id, publications: [] }) { userErrors { message } }
         }",
    );
    assert_eq!(
        operation
            .deprecations
            .iter()
            .map(|deprecation| deprecation.coordinate.as_str())
            .collect::<Vec<_>>(),
        // `input` itself is deprecated in 2026-10, for `collection`.
        [
            "CollectionInput.publications",
            "Mutation.collectionUpdate(input:)"
        ]
    );
}

#[test]
fn reports_deprecated_enum_values_written_in_the_operation() {
    let operation =
        check("{ shop { name } discountNodes(first: 1, query: \"x\") { nodes { id } } }");
    assert!(operation.deprecations.is_empty());
    let deprecated = admin()
        .types
        .iter()
        .find_map(|(name, definition)| match definition {
            shopify_api_graphql::TypeDef::Enum { deprecated, .. } if name == "CurrencyCode" => {
                deprecated.keys().next().cloned()
            }
            _ => None,
        })
        .expect("a deprecated currency code");
    let source = format!(
        "mutation {{ shopLocaleUpdate(locale: \"fr\", shopLocale: {{ published: true }}) {{ userErrors {{ message }} }} \
         priceListCreate(input: {{ name: \"x\", currency: {deprecated}, parent: {{ adjustment: {{ type: PERCENTAGE_DECREASE, value: 1 }} }} }}) {{ userErrors {{ message }} }} }}"
    );
    let operation = check(&source);
    assert_eq!(
        operation
            .deprecations
            .iter()
            .map(|deprecation| deprecation.coordinate.clone())
            .collect::<Vec<_>>(),
        [format!("CurrencyCode.{deprecated}")]
    );
}

// ─── Merging, arguments and directives ───────────────────────────────

#[test]
fn a_subfield_of_a_conditional_place_stays_conditional() {
    // `name` is only there when `$withName` is true.
    let operation =
        check("query($withName: Boolean!) { shop { id } shop @include(if: $withName) { name } }");
    assert_eq!(
        fields(child(&operation.data, "shop")),
        ["id: ID!", "name?: String!"]
    );

    // Two fragments that may each be the one that applies: a subfield is
    // certain only when both select it.
    let operation = check(
        "query($id: ID!) { node(id: $id) {
           ... on HasMetafields { metafields(first: 1) { nodes { id } } }
           ... on Product { metafields(first: 1) { nodes { id key } } }
         } }",
    );
    let metafields = child(child(&operation.data, "node"), "metafields");
    assert_eq!(fields(metafields), ["nodes: [Metafield!]!"]);
    assert_eq!(
        fields(child(metafields, "nodes")),
        ["id: ID!", "key?: String!"]
    );

    // A field selected alone under a condition keeps its subfields certain.
    let operation = check("query($b: Boolean!) { shop @include(if: $b) { name } }");
    assert_eq!(fields(&operation.data), ["shop?: Shop!"]);
    assert_eq!(fields(child(&operation.data, "shop")), ["name: String!"]);
}

#[test]
fn the_same_key_with_other_arguments_needs_an_alias() {
    assert_eq!(
        refusal("{ a: product(id: \"1\") { title } a: product(id: \"2\") { handle } }"),
        "`a` is selected twice with different arguments (`(id: \"1\")` and `(id: \"2\")`); give one of them an alias (line 1, column 33)"
    );
    // The same arguments, in another order, merge.
    check(
        "{ orders(first: 1, reverse: true) { nodes { id } } orders(reverse: true, first: 1) { nodes { name } } }",
    );
    assert_eq!(
        refusal("{ product(id: \"1\", id: \"2\") { title } }"),
        "Field `QueryRoot.product` is given the argument `id` twice (line 1, column 3)"
    );
}

/// Arguments compare as Shopify (GraphQL Ruby) compares them: by kind and
/// value, input fields in the order of the text.
#[test]
fn arguments_compare_by_kind_value_and_order() {
    let twice = |left: &str, right: &str| {
        format!(
            "mutation {{ x: flowTriggerReceive(handle: \"h\", payload: {left}) {{ userErrors {{ message }} }} x: flowTriggerReceive(handle: \"h\", payload: {right}) {{ userErrors {{ field }} }} }}"
        )
    };
    for (left, right) in [
        ("1", "1.0"),
        ("1.0", "-0.0"),
        ("0.0", "-0.0"),
        ("\"A\"", "A"),
        ("\"1\"", "1"),
        ("true", "\"true\""),
        ("null", "\"null\""),
        ("[1]", "1"),
        ("[1, 2]", "[2, 1]"),
        ("{a: 1}", "{a: 1.0}"),
        ("{a: 1}", "{a: 1, b: 2}"),
        ("{a: [1]}", "{a: 1}"),
        ("{a: \"x\"}", "{a: x}"),
    ] {
        let refused = refusal(&twice(left, right));
        assert!(
            refused.contains("`x` is selected twice with different arguments"),
            "{left} / {right}: {refused}"
        );
    }
    // Written alike: the same value.
    for (left, right) in [
        ("1.0", "1.00"),
        ("1.0", "10e-1"),
        ("\"abc\"", "\"\"\"abc\"\"\""),
        ("\"\\u0041\"", "\"A\""),
        ("{a: 1, b: [{c: 2, d: 3}]}", "{a: 1, b: [{c: 2, d: 3}]}"),
    ] {
        check(&twice(left, right));
    }
    // The fields of an input object in another order: Shopify refuses it.
    for (left, right) in [
        ("{a: 1, b: 2}", "{b: 2, a: 1}"),
        ("{n: {a: 1, b: 2}}", "{n: {b: 2, a: 1}}"),
        ("[{a: 1, b: 2}]", "[{b: 2, a: 1}]"),
    ] {
        let refused = refusal(&twice(left, right));
        assert!(
            refused.contains(
                "`x` is selected twice with the fields of an input object in another order"
            ),
            "{left} / {right}: {refused}"
        );
    }
    // An argument given or left out, as `null`.
    assert!(
        refusal(
            "mutation { x: flowTriggerReceive(handle: \"h\", payload: null) { userErrors { message } } x: flowTriggerReceive(handle: \"h\") { userErrors { field } } }"
        )
        .contains("different arguments (`(handle: \"h\", payload: null)` and `(handle: \"h\")`)")
    );
    // The messages write each value with its kind.
    assert!(
        refusal(&twice("{a: 1}", "{a: 1.0}")).contains(
            "(`(handle: \"h\", payload: {a: 1})` and `(handle: \"h\", payload: {a: 1.0})`)"
        )
    );
}

/// The order of input fields is read on the text at the position the parser
/// gives: tabs, `\r\n`, comments, block strings and other characters before
/// the fields do not throw it off.
#[test]
fn the_order_of_input_fields_is_found_after_any_text() {
    let field = |payload: &str| {
        format!(
            "x: flowTriggerReceive(handle: \"h\", payload: {payload}) {{ userErrors {{ message }} }}"
        )
    };
    let same = field("{a: 1, b: \"é\"}");
    let other = field("{b: \"é\", a: 1}");
    for before in [
        "\t\t",
        "\r\n\t# a comment ending in CRLF\r\n  ",
        "# é\n\u{feff}",
        "y: flowTriggerReceive(handle: \"\"\"line\nanother é\r\n\"\"\", payload: \"é\\\"\") { userErrors { message } }\n\t",
        "z: flowTriggerReceive(handle: \"ü\", payload: [\"\\u00e9\", {c: 1}]) { userErrors { message } } ",
    ] {
        check(&format!(
            "mutation {{ {before}{same}\r\n\t{before}{same} }}"
        ));
        let refused = refusal(&format!("mutation {{ {before}{same}\n\t{before}{other} }}"));
        assert!(
            refused.contains("in another order"),
            "{before:?}: {refused}"
        );
    }
    // Through an alias and a fragment.
    check(&format!(
        "mutation {{ ...F {same} }} fragment F on Mutation {{\t{same} }}"
    ));
}

#[test]
fn directives_come_from_the_schema() {
    // Shopify's `@idempotent`, on a mutation field.
    check(
        "mutation($key: String!) {
           inventorySetQuantities(input: { name: \"available\", reason: \"correction\", quantities: [] }) @idempotent(key: $key) {
             userErrors { message }
           }
         }",
    );
    assert_eq!(
        refusal("{ shop @inclde(if: true) { name } }"),
        "The schema has no directive `@inclde` (did you mean `@include`?) (line 1, column 8)"
    );
    assert_eq!(
        refusal("query Q @include(if: true) { shop { name } }"),
        "`@include` cannot be written on a query (line 1, column 9)"
    );
    assert_eq!(
        refusal("{ shop @include(if: true, unless: false) { name } }"),
        "`@include` has no argument `unless` (line 1, column 8)"
    );
    assert_eq!(
        refusal("{ shop @include(if: true) @include(if: false) { name } }"),
        "`@include` is written twice at the same place (line 1, column 27)"
    );
    assert_eq!(
        refusal("{ shop @skip { name } }"),
        "`@skip` requires the argument `if: Boolean!` (line 1, column 8)"
    );
    assert_eq!(
        refusal("query($flag: String!) { shop { name @include(if: $flag) } }"),
        "Variable `$flag` of type `String!` cannot be used for argument `if` of `@include`, which expects `Boolean!` (line 1, column 37)"
    );
}

/// FieldsInSetCanMerge: on two different object types, one key may name
/// different fields, or the same field with other arguments, as long as the
/// shapes of the answers agree. On the same type, or an interface, it may not.
#[test]
fn one_key_may_name_different_fields_on_different_object_types() {
    let operation = check(
        "query($id: ID!) { node(id: $id) { ... on Product { label: title } ... on Order { label: name } } }",
    );
    assert_eq!(fields(child(&operation.data, "node")), ["label?: String!"]);

    // Same type of object below: their subfields merge.
    let operation = check(
        "query($id: ID!) { node(id: $id) { ...P ...O } }
         fragment P on ProductVariant { owner: image { url } }
         fragment O on Collection { owner: image { url } }",
    );
    assert_eq!(fields(child(&operation.data, "node")), ["owner?: Image"]);

    let operation = check(
        "query($id: ID!) { node(id: $id) { ... on Product { m: metafield(key: \"a\") { value } } ... on Order { m: metafield(key: \"b\") { value } } } }",
    );
    assert_eq!(fields(child(&operation.data, "node")), ["m?: Metafield"]);

    // The same object type: the fields must be the same.
    assert!(
        refusal(
            "query($id: ID!) { node(id: $id) { ... on Product { label: title } ... on Order { label: name } ... on Product { label: handle } } }"
        )
        .contains("`label` is selected twice with different fields or types (`title: String!` and `handle: String!`)")
    );
    // Shapes that differ, even on different types.
    assert!(
        refusal(
            "query($id: ID!) { node(id: $id) { ... on Product { label: title } ... on Order { label: email } } }"
        )
        .contains("`label` is selected twice with different fields or types")
    );
}

/// Below two exclusive object types, only the shapes must agree, at every
/// depth: `a` may be `url` on one side and `originalSrc` on the other.
/// Where both sides can be in one answer, the fields must still match.
#[test]
fn keys_below_exclusive_object_types_may_name_different_fields() {
    let operation = check(
        "query($id: ID!) { node(id: $id) { ... on ProductVariant { x: image { a: url } } ... on Collection { x: image { a: originalSrc } } } }",
    );
    let node = child(&operation.data, "node");
    assert_eq!(fields(node), ["x?: Image"]);
    assert_eq!(fields(child(node, "x")), ["a: URL!"]);

    // Two levels down, the same.
    check(
        "query($id: ID!) { node(id: $id) { ... on Product { m: metafield(key: \"a\") { d: definition { n: name } } } ... on Order { m: metafield(key: \"b\") { d: definition { n: namespace } } } } }",
    );

    // The same object type on both sides: the subfields must match.
    assert!(
        refusal(
            "query($id: ID!) { node(id: $id) { ... on ProductVariant { x: image { a: url } } ... on ProductVariant { x: image { a: originalSrc } } } }"
        )
        .contains("`a` is selected twice with different fields or types (`url: URL!` and `originalSrc: URL!`)")
    );
    // An interface on one side: both may be there.
    assert!(
        refusal(
            "query($id: ID!) { node(id: $id) { ... on Node { x: id } ... on Product { x: handle } } }"
        )
        .contains("`x` is selected twice")
    );
    // Shapes still must agree below exclusive types.
    assert!(
        refusal(
            "query($id: ID!) { node(id: $id) { ... on ProductVariant { x: image { a: url } } ... on Collection { x: image { a: altText } } } }"
        )
        .contains("`a` is selected twice with different fields or types")
    );
}

/// SameResponseShape: below two exclusive object types, one key may name
/// fields of different object types when what is selected below has the
/// same shape. The subfields of both merge, each conditional unless both
/// select it.
#[test]
fn keys_below_exclusive_object_types_may_name_fields_of_different_types() {
    let operation = check(
        "{ nodes(ids: []) {
           ... on Product { price: priceRangeV2 { value: minVariantPrice { amount currencyCode } } }
           ... on Order { price: totalPriceSet { value: shopMoney { amount currencyCode } } }
         } }",
    );
    let nodes = child(&operation.data, "nodes");
    assert_eq!(fields(nodes), ["price?: ProductPriceRangeV2!"]);
    let price = child(nodes, "price");
    assert_eq!(price.types, ["ProductPriceRangeV2", "MoneyBag"]);
    assert_eq!(fields(price), ["value: MoneyV2!"]);
    assert_eq!(
        fields(child(price, "value")),
        ["amount: Decimal!", "currencyCode: CurrencyCode!"]
    );

    // A subfield one side selects alone is conditional.
    let operation = check(
        "{ nodes(ids: []) {
           ... on Product { price: priceRangeV2 { value: minVariantPrice { amount } min: minVariantPrice { amount } } }
           ... on Order { price: totalPriceSet { value: shopMoney { amount currencyCode } } }
         } }",
    );
    let price = child(child(&operation.data, "nodes"), "price");
    assert_eq!(fields(price), ["value: MoneyV2!", "min?: MoneyV2!"]);
    assert_eq!(
        fields(child(price, "value")),
        ["amount: Decimal!", "currencyCode?: CurrencyCode!"]
    );

    // Both nullable.
    check(
        "query($id: ID!) { node(id: $id) {
           ... on Product { p: compareAtPriceRange { v: maxVariantCompareAtPrice { amount } } }
           ... on Order { p: totalDiscountsSet { v: shopMoney { amount } } }
         } }",
    );
}

/// What does not have the same shape stays refused, at any depth: false
/// accepts are worse than false refusals.
#[test]
fn keys_of_different_types_need_the_same_shape() {
    let refused = |source: &str, expected: &str| {
        let refused = refusal(source);
        assert!(refused.contains(expected), "{source}\n{refused}");
    };
    let node = |product: &str, order: &str| {
        format!(
            "query($id: ID!) {{ node(id: $id) {{ ... on Product {{ {product} }} ... on Order {{ {order} }} }} }}"
        )
    };
    // A leaf of another scalar, or of another enum, below.
    refused(
        &node(
            "p: priceRangeV2 { v: minVariantPrice { amount } }",
            "p: totalPriceSet { v: shopMoney { amount: currencyCode } }",
        ),
        "`amount` is selected twice with different fields or types (`amount: Decimal!` and `currencyCode: CurrencyCode!`)",
    );
    // A leaf on one side, subfields on the other.
    refused(
        &node(
            "p: seo { t: title }",
            "p: totalPriceSet { t: shopMoney { amount } }",
        ),
        "`t` is selected twice with different fields or types (`title: String` and `shopMoney: MoneyV2!`)",
    );
    refused(
        &node("p: title", "p: totalPriceSet { shopMoney { amount } }"),
        "`p` is selected twice with different fields or types (`title: String!` and `totalPriceSet: MoneyBag!`)",
    );
    // Non-null on one side only.
    refused(
        &node(
            "p: priceRangeV2 { v: minVariantPrice { amount } }",
            "p: totalDiscountsSet { v: shopMoney { amount } }",
        ),
        "`p` is selected twice with different fields or types (`priceRangeV2: ProductPriceRangeV2!` and `totalDiscountsSet: MoneyBag`)",
    );
    // A list on one side only.
    refused(
        &node(
            "p: options { name }",
            "p: totalPriceSet { name: shopMoney { amount } }",
        ),
        "`p` is selected twice with different fields or types (`options: [ProductOption!]!` and `totalPriceSet: MoneyBag!`)",
    );
    // The same parent type: the same field, whatever the types.
    refused(
        "query($id: ID!) { node(id: $id) { ... on Product { p: priceRangeV2 { v: maxVariantPrice { amount } } } ... on Product { p: compareAtPriceRange { v: maxVariantCompareAtPrice { amount } } } } }",
        "`p` is selected twice with different fields or types",
    );
    // An interface on one side: both may be there.
    refused(
        "query($id: ID!) { node(id: $id) { ... on HasMetafields { p: metafield(key: \"a\") { id } } ... on Product { p: featuredMedia { id } } } }",
        "`p` is selected twice with different fields or types",
    );
    // The same field named on two places must be selected the same way below.
    refused(
        &node(
            "p: priceRangeV2 { v: minVariantPrice { amount } }",
            "p: totalPriceSet { v: shopMoney { amount } v: presentmentMoney { amount } }",
        ),
        "`v` is selected twice with different fields or types (`shopMoney: MoneyV2!` and `presentmentMoney: MoneyV2!`)",
    );
}

/// A field of an object type needs subfields at every place it is selected,
/// a leaf at none.
#[test]
fn every_place_of_a_field_selects_subfields_or_none() {
    assert_eq!(
        refusal("{ shop { name } shop }"),
        "Field `shop` has the type `Shop!`: select the subfields you need, `shop { … }` (line 1, column 17)"
    );
    assert_eq!(
        refusal("{ shop { name name { x } } }"),
        "Field `name` has the type `String!`, which has no subfields (line 1, column 15)"
    );
    assert_eq!(
        refusal(
            "query($id: ID!) { node(id: $id) { ... on Product { p: seo { title } } ... on Order { p: totalPriceSet } } }"
        ),
        "Field `totalPriceSet` has the type `MoneyBag!`: select the subfields you need, `totalPriceSet { … }` (line 1, column 86)"
    );
}
