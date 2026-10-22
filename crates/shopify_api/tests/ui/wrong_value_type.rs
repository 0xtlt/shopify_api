fn main() {
    let _ = shopify_api::graphql!("query($first: Int!) { products(first: $first) { nodes { id } } }", first = "ten");
}
