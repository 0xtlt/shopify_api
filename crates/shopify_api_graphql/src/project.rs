//! The Admin API version of a crate, and the schema file that goes with it.
//!
//! A crate may pin its version in its `Cargo.toml`:
//!
//! ```toml
//! [package.metadata.shopify_api]
//! api-version = "2026-07"
//! ```
//!
//! Without it, the version is the one of the schema file the crate has, kept
//! in [`PROJECT_SCHEMA_PATH`], and the latest stable one when there is no file
//! yet. `graphql!` checks the crate's operations against that schema: see
//! [`schema_plan`] for when it is downloaded.

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
    /// The crate's [`PROJECT_SCHEMA_PATH`], and the version its `Cargo.toml`
    /// declares: see [`schema_plan`].
    Project {
        /// The directory of the crate's `Cargo.toml`.
        dir: PathBuf,
        /// `dir` joined with [`PROJECT_SCHEMA_PATH`].
        path: PathBuf,
        /// The version its `Cargo.toml` declares.
        pinned: Option<String>,
    },
}

/// The schema `graphql!` uses for the crate in `manifest_dir`:
///
/// 1. the file `named` by [`SCHEMA_ENV`];
/// 2. else [`PROJECT_SCHEMA_PATH`], with the version `Cargo.toml` declares.
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
    let dir = manifest_dir
        .ok_or("no Cargo.toml of a package: the schema of a crate is kept next to it")?;
    Ok(SchemaSource::Project {
        path: dir.join(PROJECT_SCHEMA_PATH),
        pinned: configured_api_version(dir)?,
        dir: dir.to_path_buf(),
    })
}

/// What a crate's schema file holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaFile {
    Missing,
    /// A schema of `version` this version of the crate reads.
    Usable {
        version: String,
    },
    /// A file it cannot read: another format, or broken. `reason` names the
    /// file; `version` is the one it holds, when it says.
    Unusable {
        version: Option<String>,
        reason: String,
    },
}

/// What to do with a crate's schema file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaPlan {
    /// Read it as it is.
    Read,
    /// Download `version` into it, then read it.
    Download {
        version: String,
        /// No version is pinned and there is no file: `version` is the
        /// latest one, which may not be out yet on the first day of a quarter.
        latest: bool,
    },
    /// Fail the build with this message.
    Refuse(String),
}

/// What to do with the schema file of the crate in `dir`, `file`, when its
/// `Cargo.toml` pins `pinned`.
///
/// The version of a crate is the one it pins, else the one of its file: a
/// file is never replaced by a newer version on its own, so that a build
/// gives the same result whatever the date. Without either, the crate takes
/// the latest version at `now`.
///
/// Only the `primary` package, one cargo was asked to build, downloads. A
/// dependency uses the file it ships, and refuses to build when it has none
/// or when it is not the version its `Cargo.toml` pins: its author tested its
/// operations against that file, and the registry copy is not to be written.
pub fn schema_plan(
    dir: &Path,
    file: &SchemaFile,
    pinned: Option<&str>,
    primary: bool,
    now: SystemTime,
) -> SchemaPlan {
    let path = dir.join(PROJECT_SCHEMA_PATH);
    let fix = || {
        format!(
            "Run `shopify-api schema download` in `{}` (`cargo install shopify_api --features cli`), \
             and commit the file. As a dependency, a crate never downloads its schema: it uses the one it ships.",
            dir.display()
        )
    };
    match (file, pinned) {
        (SchemaFile::Usable { .. }, None) => SchemaPlan::Read,
        (SchemaFile::Usable { version }, Some(pinned)) if version == pinned => SchemaPlan::Read,
        (SchemaFile::Usable { version }, Some(pinned)) if !primary => SchemaPlan::Refuse(format!(
            "`{}` holds the Admin API {version}, but `{}` declares {pinned}. {}",
            path.display(),
            dir.join("Cargo.toml").display(),
            fix()
        )),
        (SchemaFile::Missing, _) if !primary => SchemaPlan::Refuse(format!(
            "`{}` is missing: `graphql!` needs the schema of the crate's API version there. {}",
            path.display(),
            fix()
        )),
        (SchemaFile::Unusable { reason, .. }, _) if !primary => {
            SchemaPlan::Refuse(format!("{reason}. {}", fix()))
        }
        (_, Some(pinned)) => SchemaPlan::Download {
            version: pinned.to_string(),
            latest: false,
        },
        (
            SchemaFile::Unusable {
                version: Some(version),
                ..
            },
            None,
        ) => SchemaPlan::Download {
            version: version.clone(),
            latest: false,
        },
        (_, None) => SchemaPlan::Download {
            version: latest_api_version(now),
            latest: true,
        },
    }
}

/// Shopify's public copy of the Admin schema: a GraphQL endpoint per version
/// that answers introspection without a token.
pub const PUBLIC_ADMIN_SCHEMA_ENDPOINT: &str = "https://shopify.dev/admin-graphql-direct-proxy";

/// Why the schema of a version could not be had.
#[cfg(feature = "download")]
#[cfg_attr(docsrs, doc(cfg(feature = "download")))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadError {
    /// Shopify has no Admin API of this version: it is not out yet, or its
    /// name is wrong. Downloading it again will not help.
    UnknownVersion {
        /// The version asked for.
        version: String,
        /// What Shopify answered.
        answer: String,
    },
    /// Anything else: no network, an answer that is not a schema, a file
    /// that cannot be written or read.
    Failed(String),
}

#[cfg(feature = "download")]
impl std::fmt::Display for DownloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownVersion { version, answer } => {
                write!(f, "Shopify has no Admin API version {version} ({answer})")
            }
            Self::Failed(message) => f.write_str(message),
        }
    }
}

#[cfg(feature = "download")]
impl std::error::Error for DownloadError {}

/// The introspection result in Shopify's answer to `url`, or why there is none.
#[cfg(feature = "download")]
fn introspection_in(
    version: &str,
    url: &str,
    status: u16,
    text: &str,
) -> Result<serde_json::Value, DownloadError> {
    let failed = |error: &dyn std::fmt::Display| {
        DownloadError::Failed(format!(
            "cannot download the Admin schema {version} from {url}: {error}"
        ))
    };
    let value: Option<serde_json::Value> = serde_json::from_str(text).ok();
    let message = value
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(serde_json::Value::as_str);
    if let Some(message) = message {
        // Shopify answers a version it does not have with a 400 that says so.
        if status == 400 && message.to_ascii_lowercase().contains("api version") {
            return Err(DownloadError::UnknownVersion {
                version: version.to_string(),
                answer: format!("{url} answered \"{message}\", HTTP 400"),
            });
        }
        return Err(failed(&format!("{message} (HTTP {status})")));
    }
    if !(200..300).contains(&status) {
        return Err(failed(&format!("HTTP {status}")));
    }
    value.ok_or_else(|| failed(&"the answer is not JSON"))
}

/// Downloads the Admin schema of `version` from Shopify's public endpoint.
#[cfg(feature = "download")]
#[cfg_attr(docsrs, doc(cfg(feature = "download")))]
pub fn download_public_schema(version: &str) -> Result<crate::Schema, DownloadError> {
    use std::time::Duration;

    let url = format!("{PUBLIC_ADMIN_SCHEMA_ENDPOINT}/{version}");
    let failed = |error: &dyn std::fmt::Display| {
        DownloadError::Failed(format!(
            "cannot download the Admin schema {version} from {url}: {error}"
        ))
    };
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_resolve(Some(Duration::from_secs(10)))
        .timeout_connect(Some(Duration::from_secs(10)))
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
    let value = introspection_in(version, &url, response.status().as_u16(), &text)?;
    crate::Schema::from_introspection(&value, version).map_err(|error| failed(&error))
}

/// Downloads the Admin schema of `version` into `path`, and returns it.
///
/// The file is replaced in one step: a build that reads it meanwhile sees
/// the old schema or the new one, never part of one.
#[cfg(feature = "download")]
#[cfg_attr(docsrs, doc(cfg(feature = "download")))]
pub fn download_schema_file(path: &Path, version: &str) -> Result<crate::Schema, DownloadError> {
    let schema = download_public_schema(version)?;
    write_atomically(path, &schema.to_json()).map_err(DownloadError::Failed)?;
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

/// The schema of the crate in `dir`, read with `read`, after downloading it
/// when [`schema_plan`] says so. `primary` is whether cargo was asked to
/// build this crate, rather than build it as a dependency.
///
/// A download is tried once per file and version in a process: when it
/// fails, the next operations of the build fail at once with the same error.
#[cfg(feature = "download")]
#[cfg_attr(docsrs, doc(cfg(feature = "download")))]
pub fn project_schema<S: AsRef<crate::Schema>>(
    dir: &Path,
    pinned: Option<&str>,
    primary: bool,
    now: SystemTime,
    read: impl Fn(&Path) -> Result<S, String>,
) -> Result<S, String> {
    obtain_schema(dir, pinned, primary, now, read, |path, version| {
        download_schema_file(path, version).map(drop)
    })
}

/// [`project_schema`], downloading with `download`.
#[cfg(feature = "download")]
fn obtain_schema<S: AsRef<crate::Schema>>(
    dir: &Path,
    pinned: Option<&str>,
    primary: bool,
    now: SystemTime,
    read: impl Fn(&Path) -> Result<S, String>,
    download: impl Fn(&Path, &str) -> Result<(), DownloadError>,
) -> Result<S, String> {
    let path = dir.join(PROJECT_SCHEMA_PATH);
    let (file, current) = if path.is_file() {
        match read(&path) {
            Ok(schema) => {
                let version = schema.as_ref().api_version.clone();
                (SchemaFile::Usable { version }, Some(schema))
            }
            Err(reason) => {
                let version = stored_api_version(&path);
                (SchemaFile::Unusable { version, reason }, None)
            }
        }
    } else {
        (SchemaFile::Missing, None)
    };
    match schema_plan(dir, &file, pinned, primary, now) {
        SchemaPlan::Read => current.map_or_else(|| read(&path), Ok),
        SchemaPlan::Refuse(message) => Err(message),
        SchemaPlan::Download { version, latest } => {
            download_once(&path, &version, latest, download).map_err(|error| {
                let which = match (pinned, &file) {
                    (Some(_), _) => format!("{version}, the version Cargo.toml pins"),
                    (None, SchemaFile::Missing) => format!(
                        "the latest Admin API version, {version}: Cargo.toml pins none, and there is no schema file yet"
                    ),
                    (None, _) => format!(
                        "{version}, the version of the schema file, again: this version of shopify_api cannot read it"
                    ),
                };
                download_failure(&error, &path, &which, &version, now)
            })?;
            read(&path)
        }
    }
}

/// The `apiVersion` a schema file names, whatever its format.
#[cfg(feature = "download")]
fn stored_api_version(path: &Path) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct Header {
        #[serde(rename = "apiVersion")]
        api_version: String,
    }
    let json = std::fs::read_to_string(path).ok()?;
    let header: Header = serde_json::from_str(&json).ok()?;
    Some(header.api_version)
}

/// Downloads `version` into `path` with `download`, once in the process: the
/// outcome of the first attempt is the outcome of the next ones.
///
/// For the `latest` version, an [`DownloadError::UnknownVersion`] tries the
/// version before: a new version may not be out on its first day.
#[cfg(feature = "download")]
fn download_once(
    path: &Path,
    version: &str,
    latest: bool,
    download: impl Fn(&Path, &str) -> Result<(), DownloadError>,
) -> Result<(), DownloadError> {
    type Attempts = std::collections::BTreeMap<(PathBuf, String), Result<(), DownloadError>>;
    static ATTEMPTS: std::sync::Mutex<Attempts> = std::sync::Mutex::new(Attempts::new());
    // Held during the download: the other operations of the build wait for it.
    let mut attempts = ATTEMPTS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let key = (path.to_path_buf(), version.to_string());
    if let Some(outcome) = attempts.get(&key) {
        return outcome.clone();
    }
    let outcome = match download(path, version) {
        Err(error @ DownloadError::UnknownVersion { .. }) if latest => {
            match previous_api_version(version) {
                Some(previous) => download(path, &previous),
                None => Err(error),
            }
        }
        outcome => outcome,
    };
    attempts.insert(key, outcome.clone());
    outcome
}

/// What to tell when the schema of `version`, described by `which`, cannot
/// be downloaded into `path` at `now`.
#[cfg(feature = "download")]
fn download_failure(
    error: &DownloadError,
    path: &Path,
    which: &str,
    version: &str,
    now: SystemTime,
) -> String {
    match error {
        DownloadError::UnknownVersion { .. } => {
            // On the first day of a quarter, its version may not be out yet.
            let released = latest_api_version(now);
            let suggested = match previous_api_version(version) {
                Some(previous) if released.as_str() >= version => previous,
                _ => released,
            };
            format!(
                "{error}: it is not out yet, or its name is wrong. \
                 Declare a released version in Cargo.toml:\n\n{}",
                metadata_snippet(&suggested)
            )
        }
        DownloadError::Failed(_) => format!(
            "{error}\n`graphql!` needs the schema of {which}, in `{}`. \
             It does not use another version instead: operations go to the version they were checked against. \
             Build once with network access, or run `shopify-api schema download` in the crate's directory, \
             and commit the file.",
            path.display()
        ),
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
    fn the_source_is_the_crate_file_with_its_pinned_version() {
        let project = Project::new("source");
        project.manifest(None);
        let unpinned = SchemaSource::Project {
            dir: project.0.clone(),
            path: project.0.join(PROJECT_SCHEMA_PATH),
            pinned: None,
        };
        assert_eq!(schema_source(Some(&project.0), None), Ok(unpinned));
        assert!(schema_source(None, None).is_err());

        project.manifest(Some("2026-07"));
        assert_eq!(
            schema_source(Some(&project.0), None),
            Ok(SchemaSource::Project {
                dir: project.0.clone(),
                path: project.0.join(PROJECT_SCHEMA_PATH),
                pinned: Some("2026-07".to_string()),
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
        assert!(matches!(
            schema_source(Some(&project.0), Some(OsStr::new(""))),
            Ok(SchemaSource::Project { .. })
        ));
        assert_eq!(
            schema_source(Some(&project.0), Some(OsStr::new("missing.json"))),
            Err(format!(
                "SHOPIFY_GRAPHQL_SCHEMA names `{}`, which is not a file",
                project.0.join("missing.json").display()
            ))
        );
    }

    fn usable(version: &str) -> SchemaFile {
        SchemaFile::Usable {
            version: version.to_string(),
        }
    }

    fn old_format(version: Option<&str>) -> SchemaFile {
        SchemaFile::Unusable {
            version: version.map(str::to_string),
            reason: "`/crate/graphql/admin.schema.json`: Unsupported schema file format 1, \
                     this version reads format 2"
                .to_string(),
        }
    }

    fn download(version: &str, latest: bool) -> SchemaPlan {
        SchemaPlan::Download {
            version: version.to_string(),
            latest,
        }
    }

    #[test]
    fn without_a_pin_the_file_is_the_version_whatever_the_date() {
        let dir = Path::new("/crate");
        let years_later = day(2031, 5, 6);
        for primary in [true, false] {
            assert_eq!(
                schema_plan(dir, &usable("2026-07"), None, primary, years_later),
                SchemaPlan::Read
            );
        }
        // No file and no pin: the first build of the crate takes the latest.
        assert_eq!(
            schema_plan(dir, &SchemaFile::Missing, None, true, day(2027, 2, 3)),
            download("2027-01", true)
        );
    }

    #[test]
    fn the_primary_package_downloads_its_pinned_version() {
        let dir = Path::new("/crate");
        let now = day(2026, 10, 22);
        assert_eq!(
            schema_plan(dir, &usable("2026-07"), Some("2026-07"), true, now),
            SchemaPlan::Read
        );
        assert_eq!(
            schema_plan(dir, &usable("2026-07"), Some("2026-10"), true, now),
            download("2026-10", false)
        );
        assert_eq!(
            schema_plan(dir, &SchemaFile::Missing, Some("2026-04"), true, now),
            download("2026-04", false)
        );
        // A file of an older format is downloaded again, for the same version.
        assert_eq!(
            schema_plan(dir, &old_format(Some("2026-07")), None, true, now),
            download("2026-07", false)
        );
        assert_eq!(
            schema_plan(
                dir,
                &old_format(Some("2026-07")),
                Some("2026-10"),
                true,
                now
            ),
            download("2026-10", false)
        );
        assert_eq!(
            schema_plan(dir, &old_format(None), None, true, now),
            download("2026-10", true)
        );
    }

    #[test]
    fn a_dependency_never_downloads() {
        let dir = Path::new("/registry/deplib-1.0.0");
        let now = day(2026, 10, 22);
        let refusal = |file: &SchemaFile, pinned: Option<&str>| match schema_plan(
            dir, file, pinned, false, now,
        ) {
            SchemaPlan::Refuse(message) => {
                let fix = "Run `shopify-api schema download` in `/registry/deplib-1.0.0`";
                assert!(message.contains(fix), "{message}");
                message
            }
            other => panic!("{file:?}, {pinned:?}: {other:?}"),
        };
        assert_eq!(
            schema_plan(dir, &usable("2026-07"), Some("2026-07"), false, now),
            SchemaPlan::Read
        );
        let message = refusal(&usable("2026-07"), Some("2026-10"));
        assert!(
            message.starts_with(
                "`/registry/deplib-1.0.0/graphql/admin.schema.json` holds the Admin API 2026-07, \
                 but `/registry/deplib-1.0.0/Cargo.toml` declares 2026-10."
            ),
            "{message}"
        );
        for pinned in [None, Some("2026-10")] {
            let message = refusal(&SchemaFile::Missing, pinned);
            assert!(
                message.contains("admin.schema.json` is missing"),
                "{message}"
            );
            let message = refusal(&old_format(Some("2026-07")), pinned);
            assert!(
                message.contains("Unsupported schema file format 1"),
                "{message}"
            );
        }
    }

    #[cfg(feature = "download")]
    mod obtaining {
        use std::cell::RefCell;
        use std::sync::Arc;

        use super::*;
        use crate::Schema;

        const SCHEMA: &str = r#"{"format":FORMAT,"apiVersion":"VERSION","query":"Q","types":{"Q":{"o":{"f":{"a":"Int"}}},"Int":"s"}}"#;

        fn schema(version: &str) -> String {
            SCHEMA
                .replace("FORMAT", &crate::schema::FORMAT.to_string())
                .replace("VERSION", version)
        }

        fn read(path: &Path) -> Result<Arc<Schema>, String> {
            let json = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
            Schema::from_json(&json)
                .map(Arc::new)
                .map_err(|error| error.to_string())
        }

        /// The versions asked for, and what the next downloads do.
        struct Network {
            asked: RefCell<Vec<String>>,
            online: bool,
            released: &'static [&'static str],
        }

        impl Network {
            fn new(online: bool, released: &'static [&'static str]) -> Self {
                Self {
                    asked: RefCell::new(Vec::new()),
                    online,
                    released,
                }
            }

            fn download(&self, path: &Path, version: &str) -> Result<(), DownloadError> {
                self.asked.borrow_mut().push(version.to_string());
                if !self.online {
                    return Err(DownloadError::Failed("offline".to_string()));
                }
                if !self.released.contains(&version) {
                    return Err(DownloadError::UnknownVersion {
                        version: version.to_string(),
                        answer: "HTTP 400".to_string(),
                    });
                }
                write_atomically(path, &schema(version)).map_err(DownloadError::Failed)
            }

            fn obtain(
                &self,
                project: &Project,
                primary: bool,
                now: SystemTime,
            ) -> Result<Arc<Schema>, String> {
                let pinned = configured_api_version(&project.0).unwrap();
                obtain_schema(
                    &project.0,
                    pinned.as_deref(),
                    primary,
                    now,
                    read,
                    |path, version| self.download(path, version),
                )
            }
        }

        #[test]
        fn a_dependency_reads_its_file_and_never_writes_it() {
            let project = Project::new("dependency");
            project.manifest(None);
            let file = project.write(PROJECT_SCHEMA_PATH, &schema("2026-04"));
            let network = Network::new(true, &["2026-04", "2026-07", "2026-10"]);
            let schema = network.obtain(&project, false, day(2026, 10, 22)).unwrap();
            assert_eq!(schema.api_version, "2026-04");

            project.manifest(Some("2026-10"));
            let error = network
                .obtain(&project, false, day(2026, 10, 22))
                .unwrap_err();
            assert!(error.contains("holds the Admin API 2026-04"), "{error}");
            assert!(network.asked.borrow().is_empty());
            assert!(read(&file).unwrap().api_version == "2026-04");
        }

        #[test]
        fn a_failed_download_is_tried_once_and_says_what_to_do() {
            let project = Project::new("offline");
            project.manifest(Some("2026-10"));
            project.write(PROJECT_SCHEMA_PATH, &schema("2026-07"));
            let network = Network::new(false, &[]);
            for _ in 0..3 {
                let error = network
                    .obtain(&project, true, day(2026, 10, 22))
                    .unwrap_err();
                assert!(error.starts_with("offline\n`graphql!` needs the schema of 2026-10, the version Cargo.toml pins"), "{error}");
                assert!(
                    error.contains("It does not use another version instead"),
                    "{error}"
                );
            }
            assert_eq!(*network.asked.borrow(), ["2026-10"]);
        }

        #[test]
        fn a_version_shopify_does_not_have_is_named_and_a_released_one_suggested() {
            let project = Project::new("unknown");
            let network = Network::new(true, &["2026-10"]);
            project.manifest(Some("2027-04"));
            assert_eq!(
                network
                    .obtain(&project, true, day(2026, 10, 22))
                    .unwrap_err(),
                "Shopify has no Admin API version 2027-04 (HTTP 400): it is not out yet, \
                 or its name is wrong. Declare a released version in Cargo.toml:\n\n\
                 [package.metadata.shopify_api]\napi-version = \"2026-10\""
            );
            // On the first day of a quarter, before Shopify releases its version.
            project.manifest(Some("2027-01"));
            let error = network.obtain(&project, true, day(2027, 1, 1)).unwrap_err();
            assert!(error.ends_with("api-version = \"2026-10\""), "{error}");
        }

        #[test]
        fn the_first_build_takes_the_latest_released_version() {
            let project = Project::new("first");
            project.manifest(None);
            // January 1st, before Shopify releases 2027-01.
            let network = Network::new(true, &["2026-10"]);
            let schema = network.obtain(&project, true, day(2027, 1, 1)).unwrap();
            assert_eq!(schema.api_version, "2026-10");
            assert_eq!(*network.asked.borrow(), ["2027-01", "2026-10"]);

            // Once 2027-01 is out, the file stays what it is.
            let network = Network::new(true, &["2026-10", "2027-01"]);
            let schema = network.obtain(&project, true, day(2027, 4, 2)).unwrap();
            assert_eq!(schema.api_version, "2026-10");
            assert!(network.asked.borrow().is_empty());
        }

        #[test]
        fn a_file_of_an_older_format_is_downloaded_again_for_its_version() {
            let project = Project::new("format");
            project.manifest(None);
            let format = format!("\"format\":{}", crate::schema::FORMAT);
            let old = schema("2026-07").replacen(&format, "\"format\":0", 1);
            project.write(PROJECT_SCHEMA_PATH, &old);
            let network = Network::new(true, &["2026-07", "2026-10"]);
            let schema = network.obtain(&project, true, day(2026, 10, 22)).unwrap();
            assert_eq!(schema.api_version, "2026-07");
            assert_eq!(*network.asked.borrow(), ["2026-07"]);
        }
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

    #[cfg(feature = "download")]
    #[test]
    fn a_version_shopify_does_not_have_is_told_apart() {
        let url = "https://shopify.dev/admin-graphql-direct-proxy/2027-04";
        let answer = |status, text| introspection_in("2027-04", url, status, text);
        let failed = |reason: &str| {
            Err(DownloadError::Failed(format!(
                "cannot download the Admin schema 2027-04 from {url}: {reason}"
            )))
        };
        assert_eq!(
            answer(400, r#"{"error":"Invalid API version"}"#),
            Err(DownloadError::UnknownVersion {
                version: "2027-04".to_string(),
                answer: format!("{url} answered \"Invalid API version\", HTTP 400"),
            })
        );
        assert_eq!(
            answer(401, r#"{"error":"Unauthorized"}"#),
            failed("Unauthorized (HTTP 401)")
        );
        assert_eq!(answer(503, "Service Unavailable"), failed("HTTP 503"));
        assert_eq!(answer(200, "<html>"), failed("the answer is not JSON"));
        assert_eq!(
            answer(200, r#"{"data":{}}"#),
            Ok(serde_json::json!({ "data": {} }))
        );
    }
}
