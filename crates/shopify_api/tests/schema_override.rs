//! Which schema `graphql!` takes for a library of the workspace, across cargo
//! commands that share a target directory: cargo reuses a compiled crate
//! whatever command built it, so the schema must not depend on which one did.
//!
//! Builds a small workspace with cargo, offline, in `CARGO_TARGET_TMPDIR`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Mutex, MutexGuard};

const SCHEMA_ENV: &str = "SHOPIFY_GRAPHQL_SCHEMA";

fn schema(version: &str) -> String {
    format!(
        r#"{{"format": 2, "apiVersion": "{version}", "query": "Q", "types": {{"Q": {{"o": {{"f": {{"f": "String"}}}}}}, "String": "s"}}}}"#
    )
}

/// A workspace of `app`, which prints the API version of `library`, a path
/// dependency whose own schema is 2026-04, and `override.json`, of 2026-07.
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
            "library/Cargo.toml",
            &format!(
                "[package]\nname = \"probe_library\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
                 [dependencies]\nshopify_api = {{ path = '{shopify_api}', default-features = false }}\n"
            ),
        );
        write(
            "library/src/lib.rs",
            "shopify_api::graphql! { pub mod probe = \"{ f }\"; }\n",
        );
        write("library/graphql/admin.schema.json", &schema("2026-04"));
        write("override.json", &schema("2026-07"));
        // Shared by the tests: later runs build only the probes again.
        let target = tmp.join("schema_override_target");
        Self {
            root,
            target,
            _alone: alone,
        }
    }

    fn cargo(&self, args: &[&str], with_override: bool) -> Output {
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
        let named = match with_override {
            true => self.root.join("override.json"),
            false => PathBuf::new(),
        };
        command.env(SCHEMA_ENV, named);
        command.output().unwrap()
    }

    fn succeed(&self, args: &[&str], with_override: bool) -> String {
        let output = self.cargo(args, with_override);
        assert!(
            output.status.success(),
            "cargo {args:?} failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    fn app_version(&self, with_override: bool) -> String {
        self.succeed(&["run", "-p", "probe_app"], with_override)
    }

    fn touch_library(&self) {
        let source = self.root.join("library/src/lib.rs");
        let mut text = std::fs::read_to_string(&source).unwrap();
        text.push('\n');
        std::fs::write(source, text).unwrap();
    }
}

/// A library of the workspace is local: the variable applies to it, whether
/// cargo was asked to build it or only the app, and in any order.
#[test]
fn a_local_library_takes_the_override_whatever_cargo_built_it_first() {
    let workspace = Workspace::new("local_library");
    workspace.succeed(&["build", "-p", "probe_library"], true);
    assert_eq!(workspace.app_version(true), "2026-07");
    workspace.touch_library();
    assert_eq!(workspace.app_version(true), "2026-07");
    // The variable is tracked: without it, the library is compiled again.
    assert_eq!(workspace.app_version(false), "2026-04");
    workspace.succeed(&["build", "-p", "probe_library"], false);
    assert_eq!(workspace.app_version(true), "2026-07");
}

/// A packaged library (a `Cargo.toml.orig` next to its `Cargo.toml`) keeps
/// its own schema, even when cargo is asked to build it, and never
/// downloads one it lacks.
#[test]
fn a_packaged_library_keeps_its_schema_and_never_downloads() {
    let workspace = Workspace::new("packaged_library");
    let library = workspace.root.join("library");
    std::fs::copy(library.join("Cargo.toml"), library.join("Cargo.toml.orig")).unwrap();
    workspace.succeed(&["build", "-p", "probe_library"], true);
    assert_eq!(workspace.app_version(true), "2026-04");

    std::fs::remove_file(library.join("graphql/admin.schema.json")).unwrap();
    let output = workspace.cargo(&["build", "-p", "probe_library"], false);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success());
    assert!(stderr.contains("admin.schema.json` is missing"), "{stderr}");
    assert!(!stderr.contains("cannot download"), "{stderr}");
}
