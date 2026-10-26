// A constant named like a binding of the generated code: a compile error, not a change of meaning.
#![allow(non_upper_case_globals)]
const __shopify_api_parent: usize = 0;

fn main() {
    let _ = shopify_api::graphql!(bulk("{ products { edges { node { id variants { edges { node { sku } } } } } } }"));
}
