fn main() {
    let count = 2_u8;
    let _ = shopify_api::input!(DraftOrderLineItemInput { quantity: "2", title: count, requiresShipping: 1 });
    let _ = shopify_api::input!(WeightInput { unit: GRAMS, value: 2_u64 });
}
