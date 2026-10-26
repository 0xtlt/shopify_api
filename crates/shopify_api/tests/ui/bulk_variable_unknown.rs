fn main() {
    let _ = shopify_api::graphql!(
        bulk("query($q: String) { products(query: $q) { edges { node { id } } } }"),
        filter = "x",
    );
}
