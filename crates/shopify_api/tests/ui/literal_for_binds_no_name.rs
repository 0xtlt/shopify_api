fn main() {
    let _ = shopify_api::input!(ProductUpdateInput { tags: [for _ in 0..3 => "sale"] });
}
