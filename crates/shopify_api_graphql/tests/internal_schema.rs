//! `shopify_api` checks its own operations, `graphql/operations/*.graphql`,
//! against `graphql/internal.schema.json`: the test schema cut down to them.
//! `SHOPIFY_API_BLESS=1` (`mise run schema:internal`) writes it again.

use std::path::{Path, PathBuf};

use shopify_api_graphql::{Schema, analyze};

fn runtime_crate() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../shopify_api/graphql")
}

#[test]
fn the_internal_schema_is_cut_down_to_the_operations_of_shopify_api() {
    let dir = runtime_crate();
    let full = Schema::from_json(&std::fs::read_to_string(dir.join("admin.schema.json")).unwrap())
        .unwrap();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("operations"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "graphql")
        })
        .collect();
    files.sort();
    let operations: Vec<String> = files
        .iter()
        .map(|file| std::fs::read_to_string(file).unwrap())
        .collect();
    let operations: Vec<&str> = operations.iter().map(String::as_str).collect();
    assert!(operations.len() >= 10);

    let internal = full.restricted_to(&operations).unwrap();
    for (file, operation) in files.iter().zip(&operations) {
        assert_eq!(
            analyze(&internal, operation).unwrap(),
            analyze(&full, operation).unwrap(),
            "{}",
            file.display()
        );
    }
    assert!(internal.types.len() < full.types.len() / 10);

    let path = dir.join("internal.schema.json");
    let json = internal.to_json();
    if std::env::var_os("SHOPIFY_API_BLESS").is_some() {
        std::fs::write(&path, &json).unwrap();
    }
    let written = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        written == json,
        "{} is not the schema of the operations: run `mise run schema:internal`",
        path.display()
    );
}

#[test]
fn a_restricted_schema_refuses_what_it_left_out() {
    let dir = runtime_crate();
    let full = Schema::from_json(&std::fs::read_to_string(dir.join("admin.schema.json")).unwrap())
        .unwrap();
    let internal = full.restricted_to(&["{ shop { name } }"]).unwrap();
    assert!(analyze(&internal, "{ shop { name } }").is_ok());
    let error = analyze(&internal, "{ shop { email } }").unwrap_err();
    assert!(
        error.to_string().contains("has no field `email`"),
        "{error}"
    );
    assert!(full.restricted_to(&["{ shop { nam } }"]).is_err());
}
