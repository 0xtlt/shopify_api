fn main() {
    let _ = shopify_api::graphql!(bulk("{ products { edges { node { id ... on Product { variants { edges { node { sku } } } } } } } }"));
}
