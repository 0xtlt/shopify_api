//! The Admin API version a crate declares, and the schema file that goes with it.
//!
//! A crate picks its version in its `Cargo.toml`:
//!
//! ```toml
//! [package.metadata.shopify_api]
//! api-version = "2026-07"
//! ```
//!
//! `graphql!` then checks the crate's operations against that version's
//! schema, kept in [`PROJECT_SCHEMA_PATH`]: downloaded on the first build,
//! and again when the version changes.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::{BUNDLED_API_VERSION, MINIMUM_API_VERSION, is_supported_api_version};

/// Where a crate keeps the schema of its API version, relative to its `Cargo.toml`.
pub const PROJECT_SCHEMA_PATH: &str = "graphql/admin.schema.json";

/// The environment variable naming another schema file for `graphql!`,
/// relative to the `Cargo.toml` of the crate being compiled, or absolute.
pub const SCHEMA_ENV: &str = "SHOPIFY_GRAPHQL_SCHEMA";

/// The `[package.metadata]` table `shopify_api` reads.
pub const METADATA_TABLE: &str = "shopify_api";

/// The key of the API version in [`METADATA_TABLE`].
pub const API_VERSION_KEY: &str = "api-version";

/// The lines to add to a `Cargo.toml` to select `version`.
pub fn metadata_snippet(version: &str) -> String {
    format!("[package.metadata.{METADATA_TABLE}]\n{API_VERSION_KEY} = \"{version}\"")
}

/// The Admin API version the `Cargo.toml` in `manifest_dir` declares, if any.
pub fn configured_api_version(manifest_dir: &Path) -> Result<Option<String>, String> {
    let manifest = manifest_dir.join("Cargo.toml");
    let text = match std::fs::read_to_string(&manifest) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot read `{}`: {error}", manifest.display())),
    };
    api_version_in(&text).map_err(|error| format!("`{}`: {error}", manifest.display()))
}

/// The API version a `Cargo.toml` declares, if any.
fn api_version_in(manifest: &str) -> Result<Option<String>, String> {
    let table: toml::Table = manifest.parse().map_err(|error| format!("{error}"))?;
    let value = table
        .get("package")
        .and_then(|package| package.get("metadata"))
        .and_then(|metadata| metadata.get(METADATA_TABLE))
        .and_then(|ours| ours.get(API_VERSION_KEY));
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(version) = value.as_str() else {
        return Err(format!(
            "`{API_VERSION_KEY}` in [package.metadata.{METADATA_TABLE}] must be a string, such as \"{BUNDLED_API_VERSION}\""
        ));
    };
    if !is_supported_api_version(version) {
        return Err(format!(
            "`{API_VERSION_KEY} = \"{version}\"` in [package.metadata.{METADATA_TABLE}] is not a version shopify_api calls: \
             a quarterly release from {MINIMUM_API_VERSION} on, such as \"{BUNDLED_API_VERSION}\", or \"unstable\""
        ));
    }
    Ok(Some(version.to_string()))
}

/// Where the schema of a crate comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaSource {
    /// The schema bundled with this crate.
    Bundled,
    /// A schema file, used as it is.
    File(PathBuf),
    /// The schema of the version the crate declares, in `path`: to download
    /// when the file is missing or holds another version.
    Declared {
        /// The crate's [`PROJECT_SCHEMA_PATH`].
        path: PathBuf,
        /// The version its `Cargo.toml` declares.
        version: String,
    },
}

/// The schema `graphql!` uses for the crate in `manifest_dir`, in order:
///
/// 1. the file `named` by [`SCHEMA_ENV`];
/// 2. the version declared in `Cargo.toml`, kept in [`PROJECT_SCHEMA_PATH`]
///    (the bundled schema serves the bundled version until a file exists);
/// 3. [`PROJECT_SCHEMA_PATH`] as it is, when nothing is declared;
/// 4. the bundled schema.
pub fn schema_source(
    manifest_dir: Option<&Path>,
    named: Option<&OsStr>,
) -> Result<SchemaSource, String> {
    if let Some(named) = named.filter(|value| !value.is_empty()) {
        let path = match manifest_dir {
            Some(dir) => dir.join(named),
            None => PathBuf::from(named),
        };
        if !path.is_file() {
            return Err(format!(
                "{SCHEMA_ENV} names `{}`, which is not a file",
                path.display()
            ));
        }
        return Ok(SchemaSource::File(path));
    }
    let Some(dir) = manifest_dir else {
        return Ok(SchemaSource::Bundled);
    };
    let path = dir.join(PROJECT_SCHEMA_PATH);
    Ok(match configured_api_version(dir)? {
        Some(version) if version == BUNDLED_API_VERSION && !path.is_file() => SchemaSource::Bundled,
        Some(version) => SchemaSource::Declared { path, version },
        None if path.is_file() => SchemaSource::File(path),
        None => SchemaSource::Bundled,
    })
}

/// Shopify's public copy of the Admin schema: a GraphQL endpoint per version
/// that answers introspection without a token.
pub const PUBLIC_ADMIN_SCHEMA_ENDPOINT: &str = "https://shopify.dev/admin-graphql-direct-proxy";

/// Downloads the Admin schema of `version` from Shopify's public endpoint.
#[cfg(feature = "download")]
#[cfg_attr(docsrs, doc(cfg(feature = "download")))]
pub fn download_public_schema(version: &str) -> Result<crate::Schema, String> {
    use std::time::Duration;

    let url = format!("{PUBLIC_ADMIN_SCHEMA_ENDPOINT}/{version}");
    let failed = |error: &dyn std::fmt::Display| {
        format!("cannot download the Admin schema {version} from {url}: {error}")
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .into();
    let body = serde_json::json!({ "query": crate::INTROSPECTION_QUERY }).to_string();
    let mut response = agent
        .post(&url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .header(
            "User-Agent",
            concat!("shopify_api_graphql/", env!("CARGO_PKG_VERSION")),
        )
        .send(body)
        .map_err(|error| failed(&error))?;
    let text = response
        .body_mut()
        .with_config()
        .limit(256 * 1024 * 1024)
        .read_to_string()
        .map_err(|error| failed(&error))?;
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| failed(&error))?;
    if let Some(message) = value.get("error").and_then(serde_json::Value::as_str) {
        return Err(failed(&message));
    }
    crate::Schema::from_introspection(&value, version).map_err(|error| failed(&error))
}

/// Downloads the Admin schema of `version` into `path`, and returns it.
///
/// The file is replaced in one step: a build that reads it meanwhile sees
/// the old schema or the new one, never part of one.
#[cfg(feature = "download")]
#[cfg_attr(docsrs, doc(cfg(feature = "download")))]
pub fn download_schema_file(path: &Path, version: &str) -> Result<crate::Schema, String> {
    let schema = download_public_schema(version)?;
    write_atomically(path, &schema.to_json())?;
    Ok(schema)
}

/// Writes `contents` to `path` through a temporary file in the same directory.
pub fn write_atomically(path: &Path, contents: &str) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create `{}`: {error}", parent.display()))?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(format!(".{}.tmp", std::process::id()));
    let temporary = PathBuf::from(temporary);
    std::fs::write(&temporary, contents)
        .and_then(|()| std::fs::rename(&temporary, path))
        .map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            format!("cannot write `{}`: {error}", path.display())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty directory removed when the test ends.
    struct Project(PathBuf);

    impl Project {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("shopify-api-graphql-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn write(&self, path: &str, content: &str) -> PathBuf {
            let file = self.0.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, content).unwrap();
            file
        }

        fn manifest(&self, version: Option<&str>) {
            let mut manifest = "[package]\nname = \"app\"\n".to_string();
            if let Some(version) = version {
                manifest.push_str(&format!("\n{}\n", metadata_snippet(version)));
            }
            self.write("Cargo.toml", &manifest);
        }
    }

    impl Drop for Project {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_bundled_schema_is_the_version_its_crate_declares() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let declared = configured_api_version(dir).unwrap();
        assert_eq!(declared.as_deref(), Some(BUNDLED_API_VERSION));
        assert_eq!(crate::Schema::bundled().api_version, BUNDLED_API_VERSION);
    }

    #[test]
    fn the_version_is_read_from_the_package_metadata() {
        assert_eq!(api_version_in("[package]\nname = \"app\""), Ok(None));
        assert_eq!(
            api_version_in("[package.metadata.shopify_api]\napi-version = \"2026-07\""),
            Ok(Some("2026-07".to_string()))
        );
        assert_eq!(
            api_version_in("[package.metadata.shopify_api]\napi-version = \"unstable\""),
            Ok(Some("unstable".to_string()))
        );
        let old = api_version_in("[package.metadata.shopify_api]\napi-version = \"2025-01\"")
            .unwrap_err();
        assert!(old.contains("is not a version shopify_api calls"), "{old}");
        let number =
            api_version_in("[package.metadata.shopify_api]\napi-version = 2026").unwrap_err();
        assert!(number.contains("must be a string"), "{number}");
        assert!(api_version_in("[package").is_err());
    }

    #[test]
    fn without_a_declared_version_the_file_or_the_bundled_schema_is_used() {
        let project = Project::new("undeclared");
        project.manifest(None);
        assert_eq!(
            schema_source(Some(&project.0), None),
            Ok(SchemaSource::Bundled)
        );
        let file = project.write(PROJECT_SCHEMA_PATH, "{}");
        assert_eq!(
            schema_source(Some(&project.0), None),
            Ok(SchemaSource::File(file))
        );
        assert_eq!(schema_source(None, None), Ok(SchemaSource::Bundled));
    }

    #[test]
    fn a_declared_version_selects_the_project_file() {
        let project = Project::new("declared");
        project.manifest(Some("2026-07"));
        assert_eq!(
            schema_source(Some(&project.0), None),
            Ok(SchemaSource::Declared {
                path: project.0.join(PROJECT_SCHEMA_PATH),
                version: "2026-07".to_string(),
            })
        );
    }

    #[test]
    fn the_bundled_version_needs_no_download() {
        let project = Project::new("declared-bundled");
        project.manifest(Some(BUNDLED_API_VERSION));
        assert_eq!(
            schema_source(Some(&project.0), None),
            Ok(SchemaSource::Bundled)
        );
        // A file already there is kept up to date like any other version.
        let path = project.write(PROJECT_SCHEMA_PATH, "{}");
        assert_eq!(
            schema_source(Some(&project.0), None),
            Ok(SchemaSource::Declared {
                path,
                version: BUNDLED_API_VERSION.to_string(),
            })
        );
    }

    #[test]
    fn the_environment_names_another_file() {
        let project = Project::new("named");
        project.manifest(Some("2026-07"));
        let other = project.write("schemas/unstable.json", "{}");
        assert_eq!(
            schema_source(Some(&project.0), Some(OsStr::new("schemas/unstable.json"))),
            Ok(SchemaSource::File(other.clone()))
        );
        assert_eq!(
            schema_source(None, Some(other.as_os_str())),
            Ok(SchemaSource::File(other))
        );
        assert_eq!(
            schema_source(Some(&project.0), Some(OsStr::new(""))),
            Ok(SchemaSource::Declared {
                path: project.0.join(PROJECT_SCHEMA_PATH),
                version: "2026-07".to_string(),
            })
        );
        assert_eq!(
            schema_source(Some(&project.0), Some(OsStr::new("missing.json"))),
            Err(format!(
                "SHOPIFY_GRAPHQL_SCHEMA names `{}`, which is not a file",
                project.0.join("missing.json").display()
            ))
        );
    }

    #[test]
    fn files_are_replaced_in_one_step() {
        let project = Project::new("atomic");
        let path = project.0.join("graphql/admin.schema.json");
        write_atomically(&path, "one").unwrap();
        write_atomically(&path, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
    }
}
