fn main() {
    // `graphql-parser` reads `\b` as U+0010, and the bulk text is written back from what it read.
    let _ = shopify_api::graphql!(bulk(r#"{ products(query: "a\bb") { edges { node { id } } } }"#));
}
