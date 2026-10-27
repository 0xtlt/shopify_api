fn main() {
    let _ = shopify_api::graphql!("{ orders(first: 1, sortKey: CREATED) { nodes { id } } }");
}
