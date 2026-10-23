fn main() {
    let _ = shopify_api::graphql!("query($id: ID!) { product(id: $id) { title } }", id = "1", first = 2);
}
