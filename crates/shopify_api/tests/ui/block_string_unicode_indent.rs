fn main() {
    // Lines indented with U+00A0 (two bytes) and U+2003 (three bytes): `graphql-parser` would panic.
    let _ = shopify_api::graphql!("query { products(first: 1, query: \"\"\"\n\u{a0}a\n\u{2003}b\"\"\") { nodes { id } } }");
}
