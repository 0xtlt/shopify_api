//! What `graphql!` refuses at compile time, and how it says so. The expected
//! errors are in `tests/ui/*.stderr`; `mise run ui:bless` rewrites them.

#[test]
fn graphql_refusals() {
    let cases = trybuild::TestCases::new();
    cases.compile_fail("tests/ui/*.rs");
}
