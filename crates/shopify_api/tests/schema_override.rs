//! Which schema `graphql!` takes for a library of the workspace, with
//! `schema` in its `[package.metadata.shopify_api]`, across cargo commands
//! that share a target directory: cargo reuses a compiled crate whatever
//! command built it, so the schema must not depend on which one did, nor on
//! where the package is.
//!
//! Builds a small workspace with cargo, offline, in `CARGO_TARGET_TMPDIR`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard};

/// Internal to this repository's tests: it would apply to the probes too.
const TEST_SCHEMA_ENV: &str = "SHOPIFY_API_INTERNAL_TEST_SCHEMA";

fn schema(version: &str) -> String {
    format!(
        r#"{{"format": 2, "apiVersion": "{version}", "query": "Q", "types": {{"Q": {{"o": {{"f": {{"f": "String"}}}}}}, "String": "s"}}}}"#
    )
}

/// A workspace of `app`, which prints the API version of `library`, a path
/// dependency whose default schema file is 2026-04, and
/// `schemas/override.json` in it, of 2026-07, which the `schema` key of
/// `library` may name. An `override.json` outside the library, of 2026-07
/// too, is one it may not name.
struct Workspace {
    root: PathBuf,
    target: PathBuf,
    /// The tests share a target directory, where `cargo run` of one would
    /// run the binary another just built.
    _alone: MutexGuard<'static, ()>,
}

impl Workspace {
    fn new(name: &str) -> Self {
        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
        let alone = ONE_AT_A_TIME
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let tmp = Path::new(env!("CARGO_TARGET_TMPDIR"));
        let root = tmp.join(name);
        let _ = std::fs::remove_dir_all(&root);
        let shopify_api = env!("CARGO_MANIFEST_DIR");
        let write = |path: &str, contents: &str| {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, contents).unwrap();
        };
        write(
            "Cargo.toml",
            "[workspace]\nresolver = \"3\"\nmembers = [\"app\", \"library\"]\n",
        );
        // The versions the workspace of shopify_api locks, all downloaded.
        let lock = Path::new(shopify_api).join("../../Cargo.lock");
        write("Cargo.lock", &std::fs::read_to_string(lock).unwrap());
        write(
            "app/Cargo.toml",
            "[package]\nname = \"probe_app\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
             [dependencies]\nprobe_library = { path = \"../library\" }\n",
        );
        write(
            "app/src/main.rs",
            "fn main() { print!(\"{}\", probe_library::probe::API_VERSION); }\n",
        );
        write(
            "library/src/lib.rs",
            "shopify_api::graphql! { pub mod probe = \"{ f }\"; }\n",
        );
        write("library/graphql/admin.schema.json", &schema("2026-04"));
        write("library/schemas/override.json", &schema("2026-07"));
        write("override.json", &schema("2026-07"));
        // Shared by the tests: later runs build only the probes again.
        let target = tmp.join("schema_override_target");
        let workspace = Self {
            root,
            target,
            _alone: alone,
        };
        workspace.library_metadata("");
        workspace
    }

    /// Writes the `Cargo.toml` of `library`, with these lines in its
    /// `[package.metadata.shopify_api]`.
    fn library_metadata(&self, lines: &str) {
        let shopify_api = env!("CARGO_MANIFEST_DIR");
        let manifest = format!(
            "[package]\nname = \"probe_library\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
             [dependencies]\nshopify_api = {{ path = '{shopify_api}', default-features = false }}\n\
             [package.metadata.shopify_api]\n{lines}"
        );
        std::fs::write(self.root.join("library/Cargo.toml"), manifest).unwrap();
    }

    fn with_override(&self) {
        self.library_metadata("schema = \"schemas/override.json\"\n");
    }

    fn cargo(&self, args: &[&str]) -> Output {
        let mut command = Command::new(std::env::var_os("CARGO").unwrap_or("cargo".into()));
        command
            .args(args)
            .arg("--offline")
            .arg("--quiet")
            .arg("--manifest-path")
            .arg(self.root.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", &self.target)
            .env_remove("CARGO_PRIMARY_PACKAGE");
        // Empty is no file: the `[env]` of this repository's cargo config,
        // which the probes are under, sets the variable only when it is unset.
        command.env(TEST_SCHEMA_ENV, "");
        command.output().unwrap()
    }

    fn succeed(&self, args: &[&str]) -> String {
        let output = self.cargo(args);
        assert!(
            output.status.success(),
            "cargo {args:?} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn app_version(&self) -> String {
        self.succeed(&["run", "-p", "probe_app"])
    }

    fn fail(&self, args: &[&str]) -> String {
        let output = self.cargo(args);
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        assert!(!output.status.success(), "cargo {args:?} succeeded");
        stderr
    }

    fn touch_library(&self) {
        let source = self.root.join("library/src/lib.rs");
        let mut text = std::fs::read_to_string(&source).unwrap();
        text.push('\n');
        std::fs::write(source, text).unwrap();
    }
}

/// The key applies to the library whether cargo was asked to build it or
/// only the app, and in any order. An edit of `Cargo.toml` alone compiles
/// the library again.
#[test]
fn the_key_chooses_the_schema_whatever_cargo_built_first() {
    let workspace = Workspace::new("key");
    workspace.with_override();
    workspace.succeed(&["build", "-p", "probe_library"]);
    assert_eq!(workspace.app_version(), "2026-07");
    workspace.touch_library();
    assert_eq!(workspace.app_version(), "2026-07");
    // Without the key: the default file, though the library was built last
    // with it, and as the package cargo was asked to build.
    workspace.library_metadata("");
    assert_eq!(workspace.app_version(), "2026-04");
    workspace.succeed(&["build", "-p", "probe_library"]);
    assert_eq!(workspace.app_version(), "2026-04");
    workspace.with_override();
    assert_eq!(workspace.app_version(), "2026-07");

    // `api-version` is followed too: a pin the file does not hold is an
    // error for a library built as a dependency, which never downloads.
    workspace.library_metadata("api-version = \"2026-07\"\nschema = \"schemas/override.json\"\n");
    assert_eq!(workspace.app_version(), "2026-07");
    workspace.library_metadata("api-version = \"2026-10\"\nschema = \"schemas/override.json\"\n");
    let stderr = workspace.fail(&["run", "-p", "probe_app"]);
    assert!(
        stderr.contains("override.json` holds the Admin API 2026-07"),
        "{stderr}"
    );
    assert!(stderr.contains("shopify-api schema download"), "{stderr}");
}

/// A `.cargo-ok` above the workspace makes its packages look like a cargo
/// checkout. That changes nothing to the schema chosen: only a missing file
/// is then a compile error rather than a download.
#[test]
fn an_unrelated_cargo_ok_above_does_not_change_the_schema() {
    let workspace = Workspace::new("cargo_ok_above");
    std::fs::write(workspace.root.join(".cargo-ok"), "not cargo's").unwrap();
    workspace.with_override();
    workspace.succeed(&["build", "-p", "probe_library"]);
    assert_eq!(workspace.app_version(), "2026-07");
    workspace.library_metadata("");
    assert_eq!(workspace.app_version(), "2026-04");

    std::fs::remove_file(workspace.root.join("library/graphql/admin.schema.json")).unwrap();
    let stderr = workspace.fail(&["build", "-p", "probe_library"]);
    assert!(stderr.contains("admin.schema.json` is missing"), "{stderr}");
    assert!(stderr.contains("shopify-api schema download"), "{stderr}");
    assert!(!stderr.contains("cannot download"), "{stderr}");
}

/// A packaged library (a `Cargo.toml.orig` next to its `Cargo.toml`) takes
/// the file its key names too, even when cargo is asked to build it, and
/// never downloads one it lacks.
#[test]
fn a_packaged_library_takes_its_key_and_never_downloads() {
    let workspace = Workspace::new("packaged_library");
    let library = workspace.root.join("library");
    workspace.with_override();
    std::fs::copy(library.join("Cargo.toml"), library.join("Cargo.toml.orig")).unwrap();
    workspace.succeed(&["build", "-p", "probe_library"]);
    assert_eq!(workspace.app_version(), "2026-07");

    std::fs::remove_file(library.join("schemas/override.json")).unwrap();
    let stderr = workspace.fail(&["build", "-p", "probe_library"]);
    assert!(stderr.contains("override.json` is missing"), "{stderr}");
    assert!(!stderr.contains("cannot download"), "{stderr}");
}

/// A file outside the package is refused, though it exists: `cargo package`
/// would not ship it, and the packaged crate would look for it elsewhere.
#[test]
fn a_schema_outside_the_package_is_a_compile_error() {
    let workspace = Workspace::new("outside");
    let absolute = workspace.root.join("override.json");
    // A literal string, as `library_metadata` writes the path of shopify_api.
    let absolute = format!("'{}'", absolute.display());
    for path in [
        "\"../override.json\"",
        "\"schemas/../../override.json\"",
        "\"./D:/override.json\"",
        &absolute,
    ] {
        workspace.library_metadata(&format!("schema = {path}\n"));
        let stderr = workspace.fail(&["build", "-p", "probe_library"]);
        assert!(
            stderr.contains("must name a file inside the package"),
            "{stderr}"
        );
        assert!(stderr.contains("`cargo package` ships"), "{stderr}");
    }
    workspace.library_metadata("schema = \"schemas/../schemas/override.json\"\n");
    assert_eq!(workspace.app_version(), "2026-07");
}
