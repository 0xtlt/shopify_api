fn main() {
    let _ = shopify_api::graphql!("{ products(first: 3000000000) { nodes { title } } }");
}
