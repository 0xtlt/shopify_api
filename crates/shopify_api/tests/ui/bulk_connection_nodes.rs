fn main() {
    let _ = shopify_api::graphql!(bulk("{ products { nodes { id } } }"));
}
