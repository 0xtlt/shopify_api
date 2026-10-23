The value is an [`InputObject`]: opaque, holding the JSON it is sent as, and
of one Rust type per GraphQL input object, whatever the module or the crate
of the `input!`. Values of several `input!` of one type go in one `Vec`, which
a variable of an inline [`graphql!`] or the `Variables` of a module takes as
it is:

```rust,no_run
use shopify_api::{Shopify, graphql, input};

struct Row { product_id: String, color: String, digest: Option<String> }

async fn set_colors(shopify: &Shopify, rows: &[Row]) -> shopify_api::Result<()> {
    let mut metafields = Vec::new();
    for row in rows {
        if row.color.is_empty() {
            continue;
        }
        metafields.push(input!(MetafieldsSetInput {
            ownerId: &row.product_id,
            namespace: "custom",
            key: "color",
            value: &row.color,
            type: "single_line_text_field",
            compareDigest?: row.digest.as_deref(),
        }));
    }
    shopify
        .execute(graphql!(
            "mutation($metafields: [MetafieldsSetInput!]!) {
               metafieldsSet(metafields: $metafields) { userErrors { field message } }
             }",
            metafields = metafields,
        ))
        .await?;
    Ok(())
}
```

`TypeName` is an input object of the crate's schema; another name is a
compile error, with the closest input object. The fields follow the rules of
the [literals of `graphql!`](crate::graphql!#input-values): GraphQL names, every
required field, `?:` for a field that may be left out, enum values bare or
`raw(…)`, nested objects and lists, `[for … => …]`.

A value of another input type is refused where one is expected, with a
message that names the place and the GraphQL type it expects:

```text
error[E0277]: an item of `$metafields` expects a `MetafieldsSetInput`, built with `input!(MetafieldsSetInput { … })`
```

The type given may show only as `InputObject<N>`, `N` a hash of its name.

A value that Shopify would not read as written, a `Float` that is not
finite, an `Int` beyond 32 bits, `null` for a non-null `JSON`, is kept as
refused: the operation that holds it fails with
[`Error::InvalidValue`](crate::Error::InvalidValue) before it is sent (see
[`graphql!`](crate::graphql!#values-shopify-would-not-read-as-written)).
`input!` needs no `?`.

The fields are checked against the schema of the crate where `input!` is
written. A value built in another crate, against another schema or version,
has the same Rust type as long as the GraphQL name is the same.
