//! The Admin API version of a crate, and the schema file that goes with it.
//!
//! A crate may pin its version in its `Cargo.toml`:
//!
//! ```toml
//! [package.metadata.shopify_api]
//! api-version = "2026-07"
//! ```
//!
//! Without it, the crate follows the latest stable version. Either way,
//! `graphql!` checks the crate's operations against that version's schema,
//! kept in [`PROJECT_SCHEMA_PATH`]: downloaded on the first build, and again
//! when the version changes.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::{MINIMUM_API_VERSION, is_supported_api_version};

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

/// The newest stable Admin API version at `now`: Shopify releases one on the
/// first day of each quarter, named after it (`2026-10` on October 1st).
pub fn latest_api_version(now: SystemTime) -> String {
    let seconds = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (year, month) = year_and_month(seconds / 86_400);
    format!("{year}-{:02}", (month - 1) / 3 * 3 + 1)
}

/// The quarterly release before `version`: `2026-01` before `2026-04`.
pub fn previous_api_version(version: &str) -> Option<String> {
    let (year, month) = version.split_once('-')?;
    let (year, month): (u32, u32) = (year.parse().ok()?, month.parse().ok()?);
    let previous = match month {
        1 => format!("{}-10", year.checked_sub(1)?),
        4 | 7 | 10 => format!("{year}-{:02}", month - 3),
        _ => return None,
    };
    Some(previous)
}

/// The year and month (1 to 12) of a day counted from 1970-01-01, in UTC.
fn year_and_month(days: u64) -> (u64, u64) {
    // Howard Hinnant's `civil_from_days`, for days after the epoch.
    let z = days + 719_468;
    let era = z / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + u64::from(month <= 2);
    (year, month)
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
            "`{API_VERSION_KEY}` in [package.metadata.{METADATA_TABLE}] must be a string, such as \"2026-10\""
        ));
    };
    if !is_supported_api_version(version) {
        return Err(format!(
            "`{API_VERSION_KEY} = \"{version}\"` in [package.metadata.{METADATA_TABLE}] is not a version shopify_api calls: \
             a quarterly release from {MINIMUM_API_VERSION} on, such as \"2026-10\", or \"unstable\""
        ));
    }
    Ok(Some(version.to_string()))
}

/// Where the schema of a crate comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaSource {
    /// A schema file, used as it is.
    File(PathBuf),
    /// The schema of `version`, kept in `path`: to download when the file is
    /// missing or holds another version.
    Version {
        /// The crate's [`PROJECT_SCHEMA_PATH`].
        path: PathBuf,
        /// The version its `Cargo.toml` declares, else the latest one.
        version: String,
        /// No version is declared: `version` is the latest one.
        latest: bool,
    },
}

/// The schema `graphql!` uses for the crate in `manifest_dir`, at `now`:
///
/// 1. the file `named` by [`SCHEMA_ENV`];
/// 2. the version declared in `Cargo.toml`, kept in [`PROJECT_SCHEMA_PATH`];
/// 3. else the latest version, kept there too.
pub fn schema_source(
    manifest_dir: Option<&Path>,
    named: Option<&OsStr>,
    now: SystemTime,
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
    let dir = manifest_dir
        .ok_or("no Cargo.toml of a package: the schema of a crate is kept next to it")?;
    let declared = configured_api_version(dir)?;
    Ok(SchemaSource::Version {
        path: dir.join(PROJECT_SCHEMA_PATH),
        latest: declared.is_none(),
        version: declared.unwrap_or_else(|| latest_api_version(now)),
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
        .http_status_as_error(false)
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
    let value: Option<serde_json::Value> = serde_json::from_str(&text).ok();
    let message = value
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(serde_json::Value::as_str);
    let status = response.status();
    if let Some(message) = message {
        return Err(failed(&format!("{message} (HTTP {})", status.as_u16())));
    }
    if !status.is_success() {
        return Err(failed(&format!("HTTP {}", status.as_u16())));
    }
    let value = value.ok_or_else(|| failed(&"the answer is not JSON"))?;
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

/// The schema of `version` in `path`, read with `read`: the file when it
/// holds that version, else downloaded into it.
///
/// For the `latest` version, a failed download falls back on the file as it
/// is, else on the previous version: a new version may not be out yet on its
/// first day, and a build without network keeps the schema it has.
#[cfg(feature = "download")]
#[cfg_attr(docsrs, doc(cfg(feature = "download")))]
pub fn up_to_date_schema<S: AsRef<crate::Schema>>(
    path: &Path,
    version: &str,
    latest: bool,
    read: impl Fn(&Path) -> Result<S, String>,
) -> Result<S, String> {
    let current = path.is_file().then(|| read(path).ok()).flatten();
    if let Some(current) = current {
        if current.as_ref().api_version == version {
            return Ok(current);
        }
        return match download_schema_file(path, version) {
            Ok(_) => read(path),
            Err(_) if latest => Ok(current),
            Err(error) => Err(error),
        };
    }
    match download_schema_file(path, version) {
        Ok(_) => read(path),
        Err(error) if latest => match previous_api_version(version) {
            Some(previous) if download_schema_file(path, &previous).is_ok() => read(path),
            _ => Err(error),
        },
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

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

    fn leap(year: u64) -> bool {
        (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
    }

    /// Midnight UTC of a day.
    fn day(year: u64, month: u64, day: u64) -> SystemTime {
        let mut days = 0;
        for y in 1970..year {
            days += if leap(y) { 366 } else { 365 };
        }
        let leap = leap(year);
        let lengths = [
            31,
            if leap { 29 } else { 28 },
            31,
            30,
            31,
            30,
            31,
            31,
            30,
            31,
            30,
            31,
        ];
        days += lengths[..(month - 1) as usize].iter().sum::<u64>() + day - 1;
        SystemTime::UNIX_EPOCH + Duration::from_secs(days * 86_400)
    }

    #[test]
    fn the_latest_version_is_the_one_of_the_quarter() {
        assert_eq!(latest_api_version(day(2026, 10, 1)), "2026-10");
        assert_eq!(latest_api_version(day(2026, 10, 22)), "2026-10");
        assert_eq!(latest_api_version(day(2026, 12, 31)), "2026-10");
        assert_eq!(latest_api_version(day(2027, 1, 1)), "2027-01");
        assert_eq!(latest_api_version(day(2027, 3, 31)), "2027-01");
        assert_eq!(latest_api_version(day(2028, 2, 29)), "2028-01");
        assert_eq!(latest_api_version(day(2028, 4, 1)), "2028-04");
        assert_eq!(latest_api_version(day(2030, 9, 30)), "2030-07");
    }

    #[test]
    fn the_previous_version_is_the_quarter_before() {
        assert_eq!(previous_api_version("2027-01").as_deref(), Some("2026-10"));
        assert_eq!(previous_api_version("2026-10").as_deref(), Some("2026-07"));
        assert_eq!(previous_api_version("2026-04").as_deref(), Some("2026-01"));
        assert_eq!(previous_api_version("unstable"), None);
        assert_eq!(previous_api_version("2026-05"), None);
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
    fn without_a_declared_version_the_latest_is_used() {
        let project = Project::new("latest");
        project.manifest(None);
        assert_eq!(
            schema_source(Some(&project.0), None, day(2027, 2, 3)),
            Ok(SchemaSource::Version {
                path: project.0.join(PROJECT_SCHEMA_PATH),
                version: "2027-01".to_string(),
                latest: true,
            })
        );
        assert!(schema_source(None, None, day(2027, 2, 3)).is_err());
    }

    #[test]
    fn a_declared_version_is_used() {
        let project = Project::new("declared");
        project.manifest(Some("2026-07"));
        assert_eq!(
            schema_source(Some(&project.0), None, day(2027, 2, 3)),
            Ok(SchemaSource::Version {
                path: project.0.join(PROJECT_SCHEMA_PATH),
                version: "2026-07".to_string(),
                latest: false,
            })
        );
    }

    #[test]
    fn the_environment_names_another_file() {
        let now = day(2026, 10, 22);
        let project = Project::new("named");
        project.manifest(Some("2026-07"));
        let other = project.write("schemas/unstable.json", "{}");
        assert_eq!(
            schema_source(
                Some(&project.0),
                Some(OsStr::new("schemas/unstable.json")),
                now
            ),
            Ok(SchemaSource::File(other.clone()))
        );
        assert_eq!(
            schema_source(None, Some(other.as_os_str()), now),
            Ok(SchemaSource::File(other))
        );
        assert_eq!(
            schema_source(Some(&project.0), Some(OsStr::new("")), now),
            Ok(SchemaSource::Version {
                path: project.0.join(PROJECT_SCHEMA_PATH),
                version: "2026-07".to_string(),
                latest: false,
            })
        );
        assert_eq!(
            schema_source(Some(&project.0), Some(OsStr::new("missing.json")), now),
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
