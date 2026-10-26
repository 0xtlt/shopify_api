fn main() {
    let _ = shopify_api::graphql!(bulk("{ shop { name } }"));
}
