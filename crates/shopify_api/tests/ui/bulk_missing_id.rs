fn main() {
    let _ = shopify_api::graphql!(bulk("{ products { edges { node { title variants { edges { node { sku } } } } } } }"));
}
