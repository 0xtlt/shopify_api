fn main() {
    let _ = shopify_api::graphql!(bulk("{ products { edges { node { id variants { edges { node { sku } } } media { edges { node { alt } } } collections { edges { node { title } } } metafields { edges { node { key } } } sellingPlanGroups { edges { node { name } } } } } } }"));
}
