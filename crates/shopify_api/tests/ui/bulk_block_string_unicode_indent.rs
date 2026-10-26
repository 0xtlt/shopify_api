fn main() {
    let _ = shopify_api::graphql!(bulk("{ products(query: \"\"\"\n\u{a0}a\n\u{2003}b\"\"\") { edges { node { id } } } }"));
}
