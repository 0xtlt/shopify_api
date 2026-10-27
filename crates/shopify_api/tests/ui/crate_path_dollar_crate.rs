// `$crate` means nothing in the text serde's derive reads the path from.
pub use shopify_api as runtime;

macro_rules! shop_id {
    () => {
        shopify_api::graphql!(crate = $crate::runtime, "{ shop { id } }")
    };
}

fn main() {
    let _ = shop_id!();
}
