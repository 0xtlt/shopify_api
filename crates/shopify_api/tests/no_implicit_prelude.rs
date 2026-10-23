//! The code `graphql!` writes for modules and for variables of scalars names
//! everything by its full path: it compiles without the prelude.
#![no_implicit_prelude]

extern crate shopify_api;
extern crate std;

shopify_api::graphql! {
    mod products = "query Products($first: Int!, $query: String) { products(first: $first, query: $query) { nodes { id title } } }"
}

#[test]
fn modules_and_scalar_variables_compile_without_the_prelude() {
    let variables = products::Variables {
        first: 2,
        query: ::std::option::Option::None,
    };
    let operation = products::operation(variables);
    ::std::assert_eq!(
        ::std::string::ToString::to_string(&operation.body().unwrap()["variables"]),
        r#"{"first":2}"#
    );
    let inline = shopify_api::graphql!(
        "query($first: Int!, $query: String) { products(first: $first, query: $query) { nodes { id } } }",
        first = 3,
        query = "title:shirt",
    );
    ::std::assert_eq!(
        ::std::string::ToString::to_string(&inline.body().unwrap()["variables"]),
        r#"{"first":3,"query":"title:shirt"}"#
    );
}
