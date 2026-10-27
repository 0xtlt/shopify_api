//! The code `graphql!` and `input!` generate reaches `shopify_api` through
//! `::shopify_api::__private` only, or `crate = path`: an app that depends
//! on a crate re-exporting it, and not on `shopify_api`, compiles, even when
//! that crate has a `Result`, an `Operation` or a `bulk` of its own.
//!
//! Builds a small workspace with cargo, offline, in `CARGO_TARGET_TMPDIR`.

use std::path::Path;
use std::process::Command;

/// Builds and runs `app`, which depends on `probe_facade` only, whose
/// `lib.rs` is `facade`: what the app printed.
fn run_app(name: &str, facade: &str, app: &str) -> String {
    let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
    let root = tmp.join(name);
    let _ = std::fs::remove_dir_all(&root);
    let shopify_api = Path::new(env!("CARGO_MANIFEST_DIR"));
    let write = |path: &str, contents: &str| {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    };
    write(
        "Cargo.toml",
        "[workspace]\nresolver = \"3\"\nmembers = [\"facade\", \"app\"]\n",
    );
    let lock = shopify_api.join("../../Cargo.lock");
    write("Cargo.lock", &std::fs::read_to_string(lock).unwrap());
    write(
        "facade/Cargo.toml",
        &format!(
            "[package]\nname = \"probe_facade\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
             [dependencies]\nshopify_api = {{ path = '{}', default-features = false }}\n",
            shopify_api.display()
        ),
    );
    write("facade/src/lib.rs", facade);
    // No `shopify_api` here: the app reaches it through the facade only.
    write(
        "app/Cargo.toml",
        "[package]\nname = \"probe_facade_app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
         [dependencies]\nprobe_facade = { path = \"../facade\" }\n\
         [package.metadata.shopify_api]\napi-version = \"2026-10\"\n",
    );
    write(
        "app/graphql/admin.schema.json",
        &std::fs::read_to_string(shopify_api.join("graphql/admin.schema.json")).unwrap(),
    );
    write("app/src/main.rs", app);

    let output = Command::new(std::env::var_os("CARGO").unwrap_or("cargo".into()))
        .args(["run", "--offline", "--quiet", "-p", "probe_facade_app"])
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", tmp.join(format!("{name}_target")))
        .env_remove("CARGO_PRIMARY_PACKAGE")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn the_generated_code_compiles_through_a_facade_with_names_of_its_own() {
    let printed = run_app(
        "facade",
        "pub use shopify_api::*;\n\
         // Names of its own, which shadow those of the glob.\n\
         pub type Result<T> = std::result::Result<T, String>;\n\
         pub struct Operation;\n\
         pub struct OperationKind;\n\
         pub struct InputObject;\n\
         pub struct Maybe;\n\
         pub mod bulk {}\n",
        r#"extern crate probe_facade as shopify_api;

shopify_api::graphql! {
    mod products = "query($first: Int!) { products(first: $first) { nodes { id title } } }";
    mod set = "mutation($metafields: [MetafieldsSetInput!]!) {
        metafieldsSet(metafields: $metafields) { userErrors { message } }
    }";
    mod catalog = bulk("{ products { edges { node {
        id
        variants { edges { node { id } } }
        media { edges { node { id } } }
    } } } }");
    mod tags = bulk("mutation($id: ID!, $tags: [String!]!) {
        tagsAdd(id: $id, tags: $tags) { userErrors { message } }
    }");
}

fn main() {
    let note: Option<String> = None;
    let metafield = shopify_api::input!(MetafieldsSetInput {
        ownerId: "gid://shopify/Product/1",
        namespace: "a",
        key: "b",
        type: "single_line_text_field",
        value: "c",
    });
    let _ = set::Variables { metafields: vec![metafield] };
    let _ = shopify_api::graphql!(
        "mutation($p: ProductUpdateInput!) { productUpdate(product: $p) { userErrors { message } } }",
        p = { id: "gid://shopify/Product/1", descriptionHtml?: note, status: ACTIVE },
    );
    let _ = products::operation(products::Variables { first: 1 });
    let _ = catalog::operation();
    let _ = shopify_api::graphql!(bulk("{ orders { edges { node { id } } } }"));
    let _ = tags::Variables { id: String::new(), tags: Vec::new() };
    print!("ok");
}
"#,
    );
    assert_eq!(printed, "ok");
}

#[test]
fn crate_names_the_path_of_shopify_api_in_a_crate_that_does_not_depend_on_it() {
    // A framework that re-exports the crate and its macros, with names of
    // its own at its root that the generated code must not take.
    let printed = run_app(
        "facade_crate_path",
        "pub use shopify_api;\n\
         pub use shopify_api::{graphql, input};\n\
         pub type Result<T> = std::result::Result<T, String>;\n\
         pub struct Operation;\n\
         pub mod __private {}\n",
        r#"use probe_facade::{graphql, input};

graphql! {
    crate = ::probe_facade::shopify_api;
    mod products = "query($first: Int!) { products(first: $first) { nodes { id title } } }";
    pub(crate) mod set = "mutation($metafields: [MetafieldsSetInput!]!) {
        metafieldsSet(metafields: $metafields) { userErrors { message } }
    }";
    mod catalog = bulk("{ products { edges { node {
        id
        variants { edges { node { id } } }
    } } } }");
    mod tags = bulk("mutation($id: ID!, $tags: [String!]!) {
        tagsAdd(id: $id, tags: $tags) { userErrors { message } }
    }");
}

mod nested {
    // A path from the root of the crate works too, from any module.
    pub use probe_facade::shopify_api as sa;
    probe_facade::graphql! { crate = crate::nested::sa; pub mod shop = "{ shop { name } }"; }
}

fn main() {
    let note: Option<String> = None;
    let metafield = input!(crate = ::probe_facade::shopify_api, MetafieldsSetInput {
        ownerId: "gid://shopify/Product/1",
        namespace: "a",
        key: "b",
        type: "single_line_text_field",
        value: "c",
    });
    let set = probe_facade::shopify_api::Operation::from(set::Variables { metafields: vec![metafield] });
    let update = graphql!(
        crate = ::probe_facade::shopify_api,
        "mutation($p: ProductUpdateInput!) { productUpdate(product: $p) { userErrors { message } } }",
        p = { id: "gid://shopify/Product/1", descriptionHtml?: note, status: ACTIVE },
    );
    let _ = products::operation(products::Variables { first: 1 });
    let _ = catalog::operation();
    let _ = graphql!(crate = ::probe_facade::shopify_api, bulk("{ orders { edges { node { id } } } }"));
    let _ = tags::Variables { id: String::new(), tags: Vec::new() };
    let _ = nested::shop::operation();
    print!("{}\n{}", set.variable_json("metafields").unwrap().unwrap(), update.variable_json("p").unwrap().unwrap());
}
"#,
    );
    assert_eq!(
        printed,
        "[{\"ownerId\":\"gid://shopify/Product/1\",\"namespace\":\"a\",\"key\":\"b\",\"type\":\"single_line_text_field\",\"value\":\"c\"}]\n\
         {\"id\":\"gid://shopify/Product/1\",\"status\":\"ACTIVE\"}"
    );
}
