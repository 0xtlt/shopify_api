fn main() {
    let _ = shopify_api::graphql!(bulk("{ products { edges { node { _shopifyApiTypename: title } } } }"));
}
