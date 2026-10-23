fn main() {
    let owner: Option<&str> = None;
    let _ = shopify_api::input!(MetafieldsSetInput { ownerId?: owner, key: "color", value: "blue" });
}
