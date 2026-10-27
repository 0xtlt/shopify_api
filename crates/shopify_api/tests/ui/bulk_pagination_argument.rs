fn main() {
    let _ = shopify_api::graphql!(bulk("{ products(first: 10) { edges { node { id } } } }"));
}
