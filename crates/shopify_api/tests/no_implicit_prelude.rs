//! The code `graphql!` and `input!` write for modules, variables of scalars
//! and literals of constants names everything by its full path: it compiles
//! without the prelude. A Rust expression in a literal needs the
//! `diagnostic` namespace, which `no_implicit_prelude` hides.
#![no_implicit_prelude]

extern crate shopify_api;
extern crate std;

shopify_api::graphql! {
    mod products = "query Products($first: Int!, $query: String) { products(first: $first, query: $query) { nodes { id title } } }"
}

#[test]
fn modules_variables_and_constants_compile_without_the_prelude() {
    let variables = products::Variables {
        first: 2,
        query: ::std::option::Option::None,
    };
    let operation = products::operation(variables);
    ::std::assert_eq!(
        operation.variable_json("first").unwrap(),
        ::std::option::Option::Some("2")
    );
    ::std::assert_eq!(
        operation.variable_json("query").unwrap(),
        ::std::option::Option::None
    );
    let inline = shopify_api::graphql!(
        "query($first: Int!, $query: String) { products(first: $first, query: $query) { nodes { id } } }",
        first = 3,
        query = "title:shirt",
    );
    ::std::assert_eq!(
        inline.variable_json("first").unwrap(),
        ::std::option::Option::Some("3")
    );
    ::std::assert_eq!(
        inline.variable_json("query").unwrap(),
        ::std::option::Option::Some(r#""title:shirt""#)
    );
    let product = shopify_api::input!(ProductUpdateInput { title: "a", status: ACTIVE, tags: ["b"], seo: { title: "s" } });
    let literal = shopify_api::graphql!(
        "mutation($product: ProductUpdateInput!) { productUpdate(product: $product) { userErrors { message } } }",
        product = { title: "a", status: ACTIVE, descriptionHtml: null },
    );
    ::std::assert_eq!(
        ::shopify_api::__private::serde_json::to_string(&product).unwrap(),
        r#"{"title":"a","status":"ACTIVE","tags":["b"],"seo":{"title":"s"}}"#
    );
    ::std::assert_eq!(
        literal.variable_json("product").unwrap(),
        ::std::option::Option::Some(r#"{"title":"a","status":"ACTIVE","descriptionHtml":null}"#)
    );
}
