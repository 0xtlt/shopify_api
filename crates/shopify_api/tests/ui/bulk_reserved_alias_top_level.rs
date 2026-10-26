fn main() {
    let _ = shopify_api::graphql!(bulk("{ _shopifyApiTypename: products { edges { node { id } } } }"));
}
