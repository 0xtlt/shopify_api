fn main() {
    let _ = shopify_api::input!(DraftOrderLineItemInput { quantity: 2_147_483_648_i64 });
    let _ = shopify_api::graphql!(
        "query($first: Int!) { products(first: $first) { nodes { id } } }",
        first = -2_147_483_649,
    );
}
