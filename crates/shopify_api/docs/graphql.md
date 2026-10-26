
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

The generated types, these and the `Variables` of a module, may derive more
traits in a 1.x release (`Default`, `Eq`, `Hash`…). Do not implement standard
or serde traits for them yourself: such an impl would conflict with the
derive then. Convert to your own types for traits of your own choosing.

The types of an inline operation have no name you can write: let inference
carry them, convert them to your own types, or use a [module](#named-operations).

# Variables

Values follow the operation as `name = value`. A variable that is non-null
without a default needs one; the others can be left out, and are then not sent.

A Rust value for a scalar variable is converted to the Rust type of the
variable, so the usual spellings work:

| Variable | Accepted values |
| --- | --- |
| `String`, `ID`, string-encoded scalars | `String`, `&str`, `&String`, `Cow<str>` |
| `Int` | Any integer type |
| `Float` | `f32`, `f64`, `i32`, `u32` |
| `Boolean` | `bool` |
| `JSON` | `Value`, `&Value` |
| Nullable | The bare value, or an `Option` of it: `None` sends `null` |
| List | A `Vec`, a slice, an array or a `&Vec` of accepted values |

Another type is a compile error on the value: `` `&str` is not a value for a
GraphQL variable of Rust type `i64` `` (see [`IntoVariable`]). The value is
then checked, as for [input values](#values-shopify-would-not-read-as-written).

An enum value is written bare, and checked: `sortKey = TITLE`. A string
known at run time goes through `raw(…)`, which nothing checks:
`sortKey = raw(key)`, an `Option` for a nullable variable.

An input object is written as an object literal, checked field by field
against the schema, or given as a value built by [`input!`](crate::input!):

```rust,no_run
use shopify_api::{Shopify, graphql};

struct Row { key: String, value: String }

async fn promote(shopify: &Shopify, product_id: &str, name: &str, new_vendor: Option<String>, rows: &[Row])
    -> shopify_api::Result<()> {
    let data = shopify
        .execute(graphql!(
            "mutation($product: ProductUpdateInput!) {
               productUpdate(product: $product) { userErrors { field message } }
             }",
            product = {
                id: product_id,
                title: format!("{name} (promo)"),
                status: ACTIVE,
                descriptionHtml: null,
                vendor?: new_vendor,
                seo: { title: name },
                metafields: [for row in rows if !row.value.is_empty() => {
                    namespace: "custom", key: &row.key, value: &row.value, type: "single_line_text_field",
                }],
            },
        ))
        .await?;
    for error in data.product_update.into_iter().flat_map(|payload| payload.user_errors) {
        eprintln!("{:?}: {}", error.field, error.message);
    }
    Ok(())
}
```

# Input values

A value written for an input type reads by the GraphQL type expected where
it stands:

| Form | Where | Sent |
| --- | --- | --- |
| `null` | A nullable type | `null` |
| `{ field: value, field?: option, … }` | An input object | The object, its fields in the order written |
| `[value, …]` | A list | The list; a single value is not taken for a list |
| `[for pattern in iterable => value]`, `[for … if condition => value]` | A list | One item per turn of the loop |
| `NAME` | An enum | The value, checked against the enum |
| `raw(string)` | An enum | The string, as is: any `Into<String>` |
| Any other Rust expression | Anywhere | Its value, its type checked as below |

In an object, the field names are the GraphQL ones, Rust keywords included
(`type: "single_line_text_field"`). A field that does not exist, a field
given twice, a required field left out (every one is named), `null` for a
non-null type, an unknown enum value (the valid ones are listed) and an
object or a list where the type wants another form are compile errors that
point at the token. A deprecated field or enum value is a warning that gives
Shopify's reason.

A `@oneOf` input object, such as `ProductSetIdentifiers`, takes exactly one
of its fields, written `field: value`, with a value that is not `null`:
`{ handle: "shirt" }`. No field, two fields, `field: null` and `field?: …`
are compile errors at the token, which list the fields to choose from. In
the text of an operation the same holds, and a variable given to such a field
must be non-null (`$handle: String!`).

A field is written one of four ways:

| Field | Sends |
| --- | --- |
| `title: value` | The value |
| `title: null` | `null`: Shopify clears the field |
| `title?: option` | The value of `Some`; nothing for `None`, so Shopify keeps what it has |
| `title?: maybe` | A [`Maybe`]: `Value`, `Null` or nothing for `Absent` |

An `Option` given to `title: …` does not compile: sending `null` for `None`
would clear the field. The error says to write `title?: …` or `title: null`.
A required field (non-null without a default) cannot take `?:`; a non-null
field with a default takes an `Option` there, not a [`Maybe`], whose `Null` it
refuses. On an enum field, `status?: raw(option)` takes an `Option` or a
`Maybe` of a string. A variable keeps its own rule: `name = option` sends
`null` for `None`.

A Rust expression is checked by a trait written for its place, whose error
names the GraphQL type: `` `title` of `ProductUpdateInput` expects a `String`,
not `i32` ``. It takes what a variable takes (the table above), except for an
`Option` in a field. Where an input object is expected, it takes a value of
[`input!`](crate::input!) for that type, or a `Vec`, a slice or an array of
them for a list: `` `$metafields` expects a list of `MetafieldsSetInput`, built
with `input!(MetafieldsSetInput { … })` ``. An enum takes a bare value or
`raw(…)`, never another expression.

What reads as what, when Rust could read it too:

- `{` starts an object when the braces are empty or start with `name:` or
  `name?:`, which no Rust block does: `first = { compute() }` stays a block.
- `[a, b]` is a list where a list is expected and a Rust array elsewhere;
  `[x; 3]` is an array. A variable of a list of scalars given plain Rust
  values, `ids = [a, b]`, is converted as before.
- `null` and `raw(…)` are those forms only as a whole value: a Rust variable
  named `null` is written `(null)`, a function `raw` is called as
  `self::raw(x)` or `(raw(x))`.
- A lone identifier is an enum value where an enum is expected, and a Rust
  variable everywhere else.

Nothing goes through a `serde_json::Value`: each literal is written straight
to the JSON sent, and a value of `input!` holds its JSON already.

# Values Shopify would not read as written

Every value is checked when it is built (a literal, an `input!`, the
`Variables` of a module, [`Operation::variable`]), for what JSON or GraphQL
would turn into something else:

| Value | Refused when | Reason shown |
| --- | --- | --- |
| A `Float` | NaN or infinite: JSON writes it `null`, which clears the field | `not a finite Float` |
| An `Int`, of any integer type | Outside of 32 bits, GraphQL's `Int` | `Int out of 32-bit range` |
| A non-null `JSON` | `null` at its top; `null` inside it is sent | `null for a non-null JSON field` |
| Any value | Its `Serialize` fails | `could not serialize` |

An integer literal out of range is a compile error at the literal, where
an `Int` is expected: `2_147_483_648_i64`, `-2147483649`, `0x8000_0000`,
`(3_000_000_000)`, in any base, with `_` and a suffix, negated, in
parentheses, alone or as an item of a list `[…]`. Only these forms are
checked when the crate compiles, since their meaning does not depend on what
a name resolves to. Any other value, `Some(…)`, `vec![…]`, `[x; n]`, a
computed one, or a literal its suffix's type cannot hold (under
`allow(overflowing_literals)`, `0xffff_ffff_i32` is `-1`), is checked when
it is built: it is kept as refused, and the
operation fails with [`Error::InvalidValue`] when it is sent, or when
[`Operation::body`] or [`Operation::variable_json`] reads it, before any
request. Its `Display` names the place in GraphQL terms and the reason, never
the value:

```text
invalid value at `$product.metafields[2].value`: not a finite Float
```

A serializer's message, which may quote the value, is only in
[`Error::sensitive_detail`]. Shopify's string-encoded scalars, such as
`UnsignedInt64`, are strings, and not checked. An [`InputObject`] that is
refused also fails to serialize on its own, with the same message.

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

No schema ships with `shopify_api`. Each crate that uses `graphql!` keeps the
schema of its Admin API version in `graphql/admin.schema.json`, next to its
`Cargo.toml`, and every operation of the crate is checked against it. Commit
the file: builds without network access (CI sandboxes, docs.rs) need it, and
so does a published crate.

The version is the one your `Cargo.toml` pins, and `schema` names another
file than `graphql/admin.schema.json`, relative to that `Cargo.toml`:

```toml
[package.metadata.shopify_api]
api-version = "2026-07"
schema = "schemas/admin-2026-07.json"   # optional
```

The file `schema` names must be inside the package, so that `cargo package`
ships it: an absolute path, or one that climbs above `Cargo.toml` with `..`,
is a compile error. It is a relative path with `/`, inside the package,
without `:` or `\`. A symbolic link inside the package may point elsewhere:
`cargo package` ships the content of the file it links to.

Both keys belong to the package whose `Cargo.toml` holds them: a dependency
is always checked against the file its own `Cargo.toml` names, whatever yours
says. The file `schema` names follows the same rules as the default one: the
version is the pinned one, else the one of the file; it is downloaded only
where the default one would be; one in an older format is downloaded again,
or is a compile error. Editing either key compiles the crate again.

Without a pin, the version is the one of the schema file: it does not change
when Shopify releases a new one, and a build gives the same result whatever
the date. With no file either, the first build takes the latest stable
version (the one before, on the first day of a quarter when Shopify has not
published it yet). `shopify-api schema download` moves an unpinned crate to
the latest version; pinning one is clearer.

Which file a crate is checked against depends on its `Cargo.toml` only. What
happens when that file is missing, holds another version than the pinned
one, or is in a format an older `shopify_api` wrote, depends on two more
things:

- whether cargo was asked to build the package: one of the workspace you
  build, test or run, rather than a dependency of it;
- whether the package is your own source (the packages of the workspace, and
  path dependencies inside it or outside), rather than a copy of a
  dependency: a crate from crates.io or another registry, a git dependency,
  a vendored crate, or the copy `cargo package` verifies. `graphql!` takes a
  package for a copy when a `Cargo.toml.orig` sits next to its `Cargo.toml`
  (cargo writes one into every packaged crate), when it has a
  `.cargo-checksum.json` (vendored), when its directory or one above it
  holds a `.cargo-ok` (cargo writes one at the root of every extracted
  registry source and git checkout), or when it is under `$CARGO_HOME`
  (`~/.cargo` by default), with or without symbolic links resolved.

When both hold, `graphql!` downloads the file from Shopify's public endpoint.
It tries once per build: without network access, every operation fails at
once with the reason, and `graphql!` never checks against another version
than the one the operations go to.

Otherwise it never downloads nor writes the file: it uses the one the crate
ships. When that file is missing, or does not fit, the build fails, names the
file, and names the directory where to run `shopify-api schema download`.
That includes a local library built only as a dependency of the package cargo
was asked to build (build it alone once, or download its schema), and the
verification of `cargo package`: a library that forgot to ship its schema
fails there, before it is published. A package taken for a copy by mistake
(below an unrelated `.cargo-ok`, say) fails the same way: never with another
schema.

The `shopify-api` command (`cargo install shopify_api --features cli`)
downloads the file on demand, where `schema` says: `shopify-api schema
download`, or `shopify-api schema download --shop my-shop` for the schema a
shop's token sees.

| The schema file of a crate (`schema`, else `graphql/admin.schema.json`) | Your own package, cargo was asked to build it | Your own package, built as a dependency | Packaged, vendored or checked out |
| --- | --- | --- | --- |
| Present, no pin | The file, never downloaded again. | The file. | The file. |
| Present, of the pinned version | The file. | The file. | The file. |
| Present, of another version or an older format | Downloaded again. | Compile error. | Compile error. |
| Missing | Downloaded: the pinned version, else the latest. | Compile error. | Compile error. |

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

- Subscriptions are refused, and one inline `graphql!` takes one operation.
- An error in the operation points at the whole operation; the line and
  column in its message locate the mistake inside it. An error in a value
  points at the value.
- A literal or an `input!` is checked against the schema of the crate where
  it is written. Values of `input!` from another crate have the same Rust
  type, checked against that crate's schema.
- rustc's own errors, outside the messages of `graphql!` and `input!`, name an
  input object `InputObject<N>`, `N` being a hash of its GraphQL name: when a
  `Vec` mixes two input types, for one.
- A `Maybe::Null` alone has no type to infer: write `field: null`, or
  `Maybe::<String>::Null`.
- A value is a literal, a list built by `for`, or a Rust expression: there is
  no `if`/`else` form; an expression that returns an `Option`, a `Maybe` or a
  value of `input!` covers it.
- Enum values are checked where they are written; the `String` fields of
  `Variables` and `raw(…)` are not.
- The generated code refers to `::shopify_api`: the dependency cannot be renamed.
- Under `#![no_implicit_prelude]`, a literal or an `input!` that holds a Rust
  expression does not compile: its messages are `#[diagnostic::on_unimplemented]`,
  whose `diagnostic` namespace that attribute hides. Literals of constants,
  modules and variables of scalars compile there.
