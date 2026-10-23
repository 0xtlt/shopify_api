
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
| Input object | A value of [`input!`](crate::input!), or a `Vec`, a slice or an array of them for a list |

Another type is a compile error on the value: `` `&str` is not a value for a
GraphQL variable of Rust type `i64` `` (see [`IntoVariable`]).

# Named operations

A module names the types of an operation: to share it, or to return its
data from a function. One `graphql!` holds any number of modules, each with
its attributes, visibility and source; `;` goes between them.

```rust,no_run
use shopify_api::{Shopify, graphql, input};

graphql! {
    /// Writes metafields and returns what Shopify refused.
    pub mod metafields_set = "mutation MetafieldsSet($metafields: [MetafieldsSetInput!]!) {
        metafieldsSet(metafields: $metafields) {
            metafields { id key }
            userErrors { field message code }
        }
    }";
    pub(crate) mod shop_name = "query ShopName { shop { name } }";
}

struct Row { product_id: String, color: String }

fn color_metafields(rows: &[Row]) -> Vec<metafields_set::MetafieldsSetInput> {
    rows.iter()
        .filter(|row| !row.color.is_empty())
        .map(|row| input!(MetafieldsSetInput {
            ownerId: &row.product_id,
            namespace: "custom",
            key: "color",
            value: &row.color,
            type: "single_line_text_field",
        }))
        .collect()
}

async fn set_colors(shopify: &Shopify, rows: &[Row]) -> shopify_api::Result<Vec<String>> {
    let data = shopify
        .execute(metafields_set::Variables { metafields: color_metafields(rows) })
        .await?;
    let payload = data.metafields_set.expect("a payload");
    Ok(payload.user_errors.into_iter().map(|error| error.message).collect())
}
```

| Item | |
| --- | --- |
| `QUERY` | The text of the operation. |
| `NAME`, `KIND`, `API_VERSION` | Its name, query or mutation, and the version it was checked against. |
| `Data` and nested structs | The result. A nested struct is named after its path: `DataMetafieldsSetUserErrors`. |
| `Variables` | A field per variable, absent when there is none. [`Shopify::execute`] runs it: `Operation<Data>` implements `From<Variables>`. |
| Input objects | `pub type MetafieldsSetInput = InputObject<…>` for each input object the variables may hold, the type every [`input!`](crate::input!)`(MetafieldsSetInput { … })` builds. |
| `operation(variables)` | The operation, for `execute`. Takes nothing when there are no variables. |

In `Variables`, a variable that may be left out is an `Option`, not sent when
`None`. `Variables` implements `Default` when no variable is required, so that
`..Default::default()` never leaves out a required one, and `PartialEq` when it
holds no input object. A variable of an enum is a `String` there, which
nothing checks.

Operations kept in files read the same, and editors that know GraphQL help
writing them:

```rust,ignore
graphql! {
    pub mod order = file("graphql/order.graphql");
    pub mod orders = file("graphql/orders.graphql");
}
```

# Schema and API version

No schema ships with the crate. On the first build, `graphql!` downloads the
schema of the crate's version from Shopify's public endpoint into
`graphql/admin.schema.json`, and checks every operation of the crate against
it. Later builds read the file, and download again when the version changes.
Commit the file: builds without network access (CI sandboxes, docs.rs) need
it, and so does a published crate.

The version is the one your `Cargo.toml` declares:

```toml
[package.metadata.shopify_api]
api-version = "2026-07"
```

Without it, it is the latest stable version: Shopify releases one on the first
day of each quarter (`2026-10` on October 1st), and the next build that
recompiles the crate moves to it. A field the new version removed is then a
compile error. When the new version cannot be downloaded (no network, or
Shopify has not published it yet that day), the build keeps the file it has,
else takes the version before.

The `shopify-api` command (`cargo install shopify_api --features cli`)
downloads the file again on demand: `shopify-api schema download`, or
`shopify-api schema download --shop my-shop` for the schema a shop's token
sees.

| Source, in order | |
| --- | --- |
| `SHOPIFY_GRAPHQL_SCHEMA` | Path of a schema file, relative to `Cargo.toml` or absolute. It applies to the packages cargo builds, not to their dependencies, which keep their own schema. |
| `api-version` in `[package.metadata.shopify_api]` | Kept in `graphql/admin.schema.json`, downloaded when the file is missing or holds another version. |
| The latest stable version | Kept there too, the same way. |

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
