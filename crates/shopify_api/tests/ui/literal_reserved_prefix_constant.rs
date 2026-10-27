// A constant named like a binding of the generated code: a compile error, not a change of meaning.
#![allow(non_upper_case_globals)]
const __shopify_api_value0: &str = "";

fn main() {
    let title = String::from("Shirt");
    let _ = shopify_api::input!(ProductInput { title: title });
}
