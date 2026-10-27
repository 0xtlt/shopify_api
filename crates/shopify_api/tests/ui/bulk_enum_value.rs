fn main() {
    let _ = shopify_api::graphql!(
        bulk("query($sort: ProductSortKeys) { products(sortKey: $sort) { edges { node { id } } } }"),
        sort = TITEL,
    );
}
