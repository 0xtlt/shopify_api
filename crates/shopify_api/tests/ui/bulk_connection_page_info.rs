fn main() {
    let _ = shopify_api::graphql!(bulk("{ products { edges { node { id } } pageInfo { hasNextPage } } }"));
}
