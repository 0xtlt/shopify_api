
The operation is checked while the crate compiles: a field, an argument, a
fragment, a directive or an enum value that does not exist, a missing required
argument or input field, a variable that is not declared, unused or of the
wrong type, the same response key selected with other arguments, are compile
errors on the operation, with a suggestion when a name is close to an
existing one:

```text
error: Type `Order` has no field `nam` (did you mean `name`?) (line 1, column 58)
       (checked against the Shopify Admin API 2026-10)
 --> src/orders.rs:12:13
```

What the schema deprecates is a compiler warning that gives Shopify's reason
(an error under `-D warnings`; silence it with `#[allow(deprecated)]`):

```text
warning: use of deprecated constant `…::SHOPIFY_DEPRECATION`: `Product.bodyHtml` is deprecated
         in the Shopify Admin API 2026-10: Use `descriptionHtml` instead. (line 1, column 38 of the operation)
```

# Inline operations

```rust,no_run
use shopify_api::{Shopify, graphql};

async fn recent_order_names(shopify: &Shopify, limit: u32) -> shopify_api::Result<Vec<String>> {
    let data = shopify
        .execute(graphql!(
            "query RecentOrders($first: Int!) { orders(first: $first, reverse: true) { nodes { id name } } }",
            first = limit,
        ))
        .await?;

    Ok(data.orders.nodes.into_iter().map(|order| order.name).collect())
}
```

[`Shopify::execute`] returns the contents of Shopify's `data`: read
`data.orders`, not `data.data`.

The operation is a string literal, with or without a leading `#graphql`
comment, or `file("path.graphql")`, relative to the crate's `Cargo.toml`. It
holds one query or mutation and the fragments it uses. Its name, when it has
one, is sent as `operationName`, which Shopify shows in its logs.

Reading a field the operation does not select is a compile error too:
`no field `name` on type `DataOrdersNodes``.

# Result types

Each selection is a struct with a field per selected key, in snake case:
`lineItems` is `line_items`, an alias gives its own name, `__typename` is
`typename`, a Rust keyword is written raw (`r#type`).

| GraphQL | Rust |
| --- | --- |
| `String`, `ID` | `String` |
| `Int` | `i64` |
| `Float` | `f64` |
| `Boolean` | `bool` |
| `JSON` | `serde_json::Value` |
| Shopify's other scalars: `DateTime`, `Date`, `Decimal`, `Money`, `URL`, `HTML`, `UnsignedInt64`, `BigInt`… | `String`, as Shopify sends them |
| Enum | `String` holding the value, such as `"PAID"` |
| Object, interface, union | A struct |
| `T` without `!` | `Option<T>` |
| `[T]` | `Vec<T>` |

Enum values are strings so that a value Shopify adds later does not break
reading an answer; enum values written in the operation, such as
`sortKey: CREATED_AT`, are checked.

A field selected in a fragment on another type, as `title` below, is an
`Option`, `None` for the other types; so is a field under `@include` or
`@skip`, and the subfields only such a place selects. The schema's other
directives, such as Shopify's `@idempotent(key:)` on a mutation, are checked
like arguments. A fragment on the type itself or on one of its interfaces adds plain
fields.

```rust,no_run
# async fn run(shopify: shopify_api::Shopify, ids: Vec<String>) -> shopify_api::Result<()> {
let data = shopify
    .execute(shopify_api::graphql!(
        "query($ids: [ID!]!) {
           nodes(ids: $ids) {
             __typename
             id
             ... on Product { title }
             ... on Order { name }
           }
         }",
        ids = &ids,
    ))
    .await?;

// `nodes` is `[Node]!`: a list of nullable nodes.
for node in data.nodes.iter().flatten() {
    println!("{} {} {:?} {:?}", node.typename, node.id, node.title, node.name);
}
# Ok(())
# }
```

The types derive `Debug`, `Clone`, `PartialEq`, `Serialize` and
`Deserialize`; they serialize under the names of the GraphQL fields.

The types of an inline operation have no name you can write: let inference
carry them, convert them to your own types, or use a [module](#named-operations).

# Variables

Values follow the operation as `name = value`. A variable that is non-null
without a default needs one; the others can be left out, and are then not sent.
A value is converted to the Rust type of its variable, so the usual
spellings work:

| Variable | Accepted values |
| --- | --- |
| `String`, `ID`, enums, string-encoded scalars | `String`, `&str`, `&String`, `Cow<str>` |
| `Int` | Any integer type |
| `Float` | `f32`, `f64`, `i32`, `u32` |
| `Boolean` | `bool` |
| `JSON` | `Value`, `&Value` |
| Nullable | The bare value, or an `Option` of it |
| List | A `Vec`, a slice, an array or a `&Vec` of accepted values |
| Input object | Any serializable value, such as `json!({ … })`, checked by nothing: use a module to check its fields |

Another type is a compile error on the value: `` `&str` is not a value for a
GraphQL variable of Rust type `i64` `` (see [`IntoVariable`]).

# Named operations

A module names the types of an operation: to share it, to return its data
from a function, or to build input objects whose fields are checked.

```rust,no_run
use shopify_api::{Shopify, graphql};

graphql! {
    /// Writes metafields and returns what Shopify refused.
    pub mod metafields_set = "mutation MetafieldsSet($metafields: [MetafieldsSetInput!]!) {
        metafieldsSet(metafields: $metafields) {
            metafields { id key }
            userErrors { field message code }
        }
    }";
}

async fn set_color(shopify: &Shopify, product_id: &str) -> shopify_api::Result<Vec<String>> {
    let variables = metafields_set::Variables {
        metafields: vec![metafields_set::MetafieldsSetInput {
            owner_id: product_id.to_string(),
            namespace: Some("custom".to_string()),
            key: "color".to_string(),
            value: "blue".to_string(),
            ..Default::default()
        }],
    };
    let data: metafields_set::Data = shopify.execute(metafields_set::operation(variables)).await?;

    let payload = data.metafields_set.expect("a payload");
    Ok(payload.user_errors.into_iter().map(|error| error.message).collect())
}
```

| Item | |
| --- | --- |
| `QUERY` | The text of the operation. |
| `NAME`, `KIND`, `API_VERSION` | Its name, query or mutation, and the version it was checked against. |
| `Data` and nested structs | The result. A nested struct is named after its path: `DataMetafieldsSetUserErrors`. |
| `Variables` | A field per variable. Absent when there is none. |
| Input objects | A struct per input object the variables use, named as in the schema. |
| `operation(variables)` | The operation, for `execute`. Takes nothing when there are no variables. |

In `Variables` and in input objects, a field that may be left out is an
`Option`, not sent when `None`; the structs implement `Default`. A deprecated
input field is `#[deprecated]`: setting it warns. An explicit `null` cannot be
written this way: use [`Shopify::graphql`] with a `json!` value for the few
mutations that need one.

Operations kept in files read the same, and editors that know GraphQL help
writing them:

```rust,ignore
graphql! {
    pub mod order = file("graphql/order.graphql");
}
```

# Schema and API version

The schema bundled with the crate is the one of [`ApiVersion::DEFAULT`],
2026-10. Nothing has to be downloaded to use `graphql!`.

For another version, download its schema next to your `Cargo.toml`:

```bash
cargo install shopify_api --features cli
shopify-api schema download --api-version 2026-07
```

This writes `graphql/admin.schema.json` (from Shopify's public endpoint, or
through a shop with `--shop`), and `graphql!` then checks every operation of
the crate against it. Commit the file: builds need it.

| Source, in order | |
| --- | --- |
| `SHOPIFY_GRAPHQL_SCHEMA` | Path of a schema file, relative to `Cargo.toml` or absolute. It applies to the packages cargo builds, not to their dependencies, which keep their own schema. |
| `graphql/admin.schema.json` | Written by `shopify-api schema download`. |
| The bundled schema | [`ApiVersion::DEFAULT`]. |

An operation goes to the version of the schema it was checked against: a
crate whose schema is 2026-07 calls the 2026-07 endpoint, with nothing to
configure. A client created with [`ShopifyBuilder::api_version`] sends
everything to that version instead, and logs a warning for an operation
checked against another.

`execute` fails when `data` does not fit the operation, with the path of the
first mismatch: `` Shopify's answer does not match the operation at `orders.nodes[0]`: missing field `name` ``.
That happens when a shop runs another version than the schema, or when a test
scripts an answer that leaves out a non-null field.

# Limits

- Subscriptions are refused, and one `graphql!` takes one operation.
- The error points at the whole operation; the line and column in its message
  locate the mistake inside it.
- The generated code refers to `::shopify_api`: the dependency cannot be renamed.
