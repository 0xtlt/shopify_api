//! The Admin API version of a crate, and the schema file that goes with it.
//!
//! A crate may pin its version, and name its schema file, in its `Cargo.toml`:
//!
//! ```toml
//! [package.metadata.shopify_api]
//! api-version = "2026-07"
//! schema = "schemas/admin-2026-07.json"
//! ```
//!
//! `schema` must name a file inside the package, which `cargo package`
//! ships: see [`inside_package`]. Without it, the file is
//! [`PROJECT_SCHEMA_PATH`]. Without
//! `api-version`, the version is the one of that file, and the latest stable
//! one when there is no file yet. `graphql!` checks the crate's operations
//! against that schema: see [`schema_plan`] for when it is downloaded.
//!
//! Both keys belong to the package: which schema a crate is checked against
//! depends on its own `Cargo.toml` and files only, never on where it is, nor
//! on the cargo command that built it.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::{MINIMUM_API_VERSION, is_supported_api_version};

/// Where a crate keeps the schema of its API version, relative to its `Cargo.toml`.
pub const PROJECT_SCHEMA_PATH: &str = "graphql/admin.schema.json";

/// Internal to this repository's tests, not part of the API: a schema file
/// `graphql!` uses for every package, relative to the `Cargo.toml` of the
/// crate being compiled, or absolute, as it is. This repository's
/// `.cargo/config.toml` sets it, for the projects `trybuild` compiles, which
/// have no schema file. It applies to every package, with no exception, so
/// that its effect does not depend on where a package is.
pub const TEST_SCHEMA_ENV: &str = "SHOPIFY_API_INTERNAL_TEST_SCHEMA";

/// The `[package.metadata]` table `shopify_api` reads.
pub const METADATA_TABLE: &str = "shopify_api";

/// The key of the API version in [`METADATA_TABLE`].
pub const API_VERSION_KEY: &str = "api-version";

/// The key of the schema file in [`METADATA_TABLE`], relative to the
/// package's `Cargo.toml` and inside the package ([`inside_package`]);
/// [`PROJECT_SCHEMA_PATH`] without it.
pub const SCHEMA_KEY: &str = "schema";

/// The lines to add to a `Cargo.toml` to select `version`.
pub fn metadata_snippet(version: &str) -> String {
    format!("[package.metadata.{METADATA_TABLE}]\n{API_VERSION_KEY} = \"{version}\"")
}

/// The lines to add to a `Cargo.toml` to select `version` and the schema
/// file at `path`, relative to it: `path` is written with `/` separators, as
/// a TOML string, so that any file name reads back as it is.
pub fn schema_snippet(version: &str, path: &Path) -> String {
    let path = path
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    format!(
        "{}\n{SCHEMA_KEY} = {}",
        metadata_snippet(version),
        toml::Value::from(path)
    )
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

/// What the `[package.metadata.shopify_api]` of a `Cargo.toml` declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackageConfig {
    /// [`API_VERSION_KEY`].
    pub api_version: Option<String>,
    /// [`SCHEMA_KEY`], relative to the `Cargo.toml`, normalized by
    /// [`inside_package`].
    pub schema: Option<PathBuf>,
}

impl PackageConfig {
    /// The schema file of the package in `dir`: [`SCHEMA_KEY`], else
    /// [`PROJECT_SCHEMA_PATH`].
    pub fn schema_path(&self, dir: &Path) -> PathBuf {
        dir.join(
            self.schema
                .as_deref()
                .unwrap_or(Path::new(PROJECT_SCHEMA_PATH)),
        )
    }
}

/// What the `Cargo.toml` in `manifest_dir` declares; nothing when there is none.
pub fn package_config(manifest_dir: &Path) -> Result<PackageConfig, String> {
    let manifest = manifest_dir.join("Cargo.toml");
    let text = match std::fs::read_to_string(&manifest) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PackageConfig::default());
        }
        Err(error) => return Err(format!("cannot read `{}`: {error}", manifest.display())),
    };
    config_in(&text).map_err(|error| format!("`{}`: {error}", manifest.display()))
}

/// The Admin API version the `Cargo.toml` in `manifest_dir` declares, if any.
pub fn configured_api_version(manifest_dir: &Path) -> Result<Option<String>, String> {
    package_config(manifest_dir).map(|config| config.api_version)
}

/// What a `Cargo.toml` declares.
fn config_in(manifest: &str) -> Result<PackageConfig, String> {
    let table: toml::Table = manifest.parse().map_err(|error| format!("{error}"))?;
    let ours = table
        .get("package")
        .and_then(|package| package.get("metadata"))
        .and_then(|metadata| metadata.get(METADATA_TABLE));
    let Some(ours) = ours else {
        return Ok(PackageConfig::default());
    };
    Ok(PackageConfig {
        api_version: api_version_in(ours.get(API_VERSION_KEY))?,
        schema: schema_in(ours.get(SCHEMA_KEY))?,
    })
}

/// The schema file [`SCHEMA_KEY`] names, if any.
fn schema_in(value: Option<&toml::Value>) -> Result<Option<PathBuf>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    match value.as_str() {
        Some(path) if !path.is_empty() => inside_package(path).map(Some),
        _ => Err(format!(
            "`{SCHEMA_KEY}` in [package.metadata.{METADATA_TABLE}] must be the path of a schema file, \
             relative to Cargo.toml, such as \"graphql/admin.schema.json\""
        )),
    }
}

/// `path`, relative to a package's `Cargo.toml`, normalized, when it names a
/// file inside the package: `cargo package` ships only the package's files,
/// so a packaged crate would look for any other file at another place, and
/// fail, or find another schema there.
///
/// `path` is portable, since a published crate may be built on any system:
/// it is read with `/` separators, and no segment may hold a `:` or a `\`,
/// a drive or a separator on Windows and part of a name elsewhere.
///
/// The check is lexical: `.`, empty segments and `a/..` are removed, and the
/// path must not be absolute, nor climb above the package. A symbolic link
/// inside the package that points outside it is accepted, on purpose:
/// `cargo package` ships the content of a linked file. The path is returned
/// normalized, so that the file opened is the one checked, even when `a` in
/// `a/..` is a link to a directory; on every system, it holds only the
/// names checked, as plain components: no drive, root, `.` nor `..`.
pub fn inside_package(path: &str) -> Result<PathBuf, String> {
    use std::path::Component;
    let refused = || {
        format!(
            "`{SCHEMA_KEY} = {}` in [package.metadata.{METADATA_TABLE}] must name a file inside the package, \
             as a relative path with `/` separators and no `:` or `\\`, such as \"graphql/admin.schema.json\": \
             `cargo package` ships only the package's files, and a published crate may be built on any system",
            toml::Value::from(path)
        )
    };
    if path.starts_with('/') {
        return Err(refused());
    }
    let mut names = Vec::new();
    for segment in path.split('/') {
        if segment.contains([':', '\\']) {
            return Err(refused());
        }
        match segment {
            "" | "." => {}
            ".." => {
                if names.pop().is_none() {
                    return Err(refused());
                }
            }
            name => names.push(name),
        }
    }
    if names.is_empty() {
        return Err(refused());
    }
    let normal: PathBuf = names.iter().collect();
    // What this system makes of the names: each one a plain component.
    let plain = normal
        .components()
        .eq(names.iter().map(|name| Component::Normal(OsStr::new(name))));
    if !plain {
        return Err(refused());
    }
    Ok(normal)
}

/// The API version [`API_VERSION_KEY`] declares, if any.
fn api_version_in(value: Option<&toml::Value>) -> Result<Option<String>, String> {
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
    /// A schema file, used as it is: the one [`TEST_SCHEMA_ENV`] names.
    File(PathBuf),
    /// The schema file of the crate, and the version its `Cargo.toml`
    /// declares: see [`schema_plan`].
    Project {
        /// The directory of the crate's `Cargo.toml`.
        dir: PathBuf,
        /// [`PackageConfig::schema_path`].
        path: PathBuf,
        /// The version its `Cargo.toml` declares.
        pinned: Option<String>,
    },
}

/// The schema `graphql!` uses for the crate in `manifest_dir`:
///
/// 1. the file `test_schema` names, the value of [`TEST_SCHEMA_ENV`]
///    (internal to this repository's tests);
/// 2. else the one [`SCHEMA_KEY`] names, else [`PROJECT_SCHEMA_PATH`], with
///    the version `Cargo.toml` declares.
pub fn schema_source(
    manifest_dir: Option<&Path>,
    test_schema: Option<&OsStr>,
) -> Result<SchemaSource, String> {
    if let Some(named) = test_schema.filter(|value| !value.is_empty()) {
        let path = match manifest_dir {
            Some(dir) => dir.join(named),
            None => PathBuf::from(named),
        };
        if !path.is_file() {
            return Err(format!(
                "{TEST_SCHEMA_ENV} names `{}`, which is not a file",
                path.display()
            ));
        }
        return Ok(SchemaSource::File(path));
    }
    let dir = manifest_dir
        .ok_or("no Cargo.toml of a package: the schema of a crate is kept next to it")?;
    let config = package_config(dir)?;
    Ok(SchemaSource::Project {
        path: config.schema_path(dir),
        pinned: config.api_version,
        dir: dir.to_path_buf(),
    })
}

/// Whether the package in `dir` is local: the user's own source, in a
/// workspace or a path dependency outside it, rather than a packaged,
/// vendored or checked-out copy of a dependency. It only gates downloads,
/// see [`may_download`].
///
/// A package is not local when:
/// - its directory holds a `Cargo.toml.orig`, which cargo writes into every
///   packaged crate (registry sources, and the copy `cargo package`
///   verifies), or a `.cargo-checksum.json` (vendored);
/// - its directory or one of its ancestors holds a `.cargo-ok`, which cargo
///   writes at the root of every extracted registry source and of every git
///   checkout, where a git dependency may be a subdirectory;
/// - it is under `cargo_home`, where cargo keeps registry sources and git
///   checkouts.
///
/// Paths are compared both as given and with symbolic links resolved, so a
/// `$CARGO_HOME`, or a `git` or `registry` directory in it, that links to
/// another disk still counts.
///
/// These are heuristics: an unrelated `.cargo-ok` above a workspace, for
/// one, makes it look not local. That is acceptable because the answer
/// never chooses a schema: it only lets a missing or outdated file be
/// downloaded, or turns that into a compile error that names the file and
/// the command to run. A mistake makes a build fail loudly, never compile
/// against another schema.
pub fn is_local_package(dir: &Path, cargo_home: Option<&Path>) -> bool {
    if dir.join("Cargo.toml.orig").exists() || dir.join(".cargo-checksum.json").exists() {
        return false;
    }
    let dirs = both_forms(dir);
    let in_cargo_checkout = dirs
        .iter()
        .any(|dir| dir.ancestors().any(|dir| dir.join(".cargo-ok").exists()));
    if in_cargo_checkout {
        return false;
    }
    let Some(home) = cargo_home else {
        return true;
    };
    let homes = both_forms(home);
    !dirs
        .iter()
        .any(|dir| homes.iter().any(|home| dir.starts_with(home)))
}

/// `path` made absolute, and also with symbolic links resolved when it
/// exists.
fn both_forms(path: &Path) -> Vec<PathBuf> {
    let logical = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut forms = vec![logical];
    if let Ok(canonical) = forms[0].canonicalize()
        && canonical != forms[0]
    {
        forms.push(canonical);
    }
    forms
}

/// The directory where cargo keeps registry sources and git checkouts:
/// `$CARGO_HOME`, else `.cargo` in the home directory.
pub fn cargo_home() -> Option<PathBuf> {
    let non_empty = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
    if let Some(home) = non_empty("CARGO_HOME") {
        return Some(PathBuf::from(home));
    }
    non_empty("HOME")
        .or_else(|| non_empty("USERPROFILE"))
        .map(|home| PathBuf::from(home).join(".cargo"))
}

/// Whether `graphql!` may download the schema file of the package in `dir`,
/// one cargo was asked to build when `primary` (`CARGO_PRIMARY_PACKAGE`):
/// only a local one ([`is_local_package`]).
///
/// Neither input is tracked by cargo, and neither needs to be: they decide
/// whether a missing or outdated file is downloaded or a compile error,
/// never which file is read. A compiled crate read the same file whatever
/// they were.
pub fn may_download(dir: &Path, cargo_home: Option<&Path>, primary: bool) -> bool {
    primary && is_local_package(dir, cargo_home)
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

/// What to do with `file`, the schema file at `path` of the crate in `dir`,
/// when its `Cargo.toml` pins `pinned`.
///
/// The version of a crate is the one it pins, else the one of its file: a
/// file is never replaced by a newer version on its own, so that a build
/// gives the same result whatever the date. Without either, the crate takes
/// the latest version at `now`.
///
/// Only a package that [`may_download`] downloads: a local one cargo was
/// asked to build. Any other uses the file it ships, and
/// refuses to build when it has none or when it is not the version its
/// `Cargo.toml` pins: its author tested its operations against that file,
/// and a registry, vendored or packaged copy is not to be written.
pub fn schema_plan(
    dir: &Path,
    path: &Path,
    file: &SchemaFile,
    pinned: Option<&str>,
    may_download: bool,
    now: SystemTime,
) -> SchemaPlan {
    let fix = || {
        format!(
            "Run `shopify-api schema download` in `{}` (`cargo install shopify_api --features cli`), \
             and commit the file. A crate downloads its schema only as a local package cargo was asked to build: \
             as a dependency, packaged, vendored, under $CARGO_HOME or below a `.cargo-ok`, it uses the one it ships.",
            dir.display()
        )
    };
    match (file, pinned) {
        (SchemaFile::Usable { .. }, None) => SchemaPlan::Read,
        (SchemaFile::Usable { version }, Some(pinned)) if version == pinned => SchemaPlan::Read,
        (SchemaFile::Usable { version }, Some(pinned)) if !may_download => {
            SchemaPlan::Refuse(format!(
                "`{}` holds the Admin API {version}, but `{}` declares {pinned}. {}",
                path.display(),
                dir.join("Cargo.toml").display(),
                fix()
            ))
        }
        (SchemaFile::Missing, _) if !may_download => SchemaPlan::Refuse(format!(
            "`{}` is missing: `graphql!` needs the schema of the crate's API version there. {}",
            path.display(),
            fix()
        )),
        (SchemaFile::Unusable { reason, .. }, _) if !may_download => {
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

/// The schema of the crate in `dir`, read from `path` with `read`, after
/// downloading it when [`schema_plan`] says so, with `may_download` from
/// [`may_download()`]. `on_download`
/// is called with the file and the version before each download, to show
/// progress.
///
/// A download is tried once per file and version in a process: when it
/// fails, the next operations of the build fail at once with the same error.
#[cfg(feature = "download")]
#[cfg_attr(docsrs, doc(cfg(feature = "download")))]
pub fn project_schema<S: AsRef<crate::Schema>>(
    dir: &Path,
    path: &Path,
    pinned: Option<&str>,
    may_download: bool,
    now: SystemTime,
    read: impl Fn(&Path) -> Result<S, String>,
    on_download: impl Fn(&Path, &str),
) -> Result<S, String> {
    obtain_schema(
        dir,
        path,
        pinned,
        may_download,
        now,
        read,
        |path, version| {
            on_download(path, version);
            download_schema_file(path, version).map(drop)
        },
    )
}

/// [`project_schema`], downloading with `download`.
#[cfg(feature = "download")]
fn obtain_schema<S: AsRef<crate::Schema>>(
    dir: &Path,
    path: &Path,
    pinned: Option<&str>,
    may_download: bool,
    now: SystemTime,
    read: impl Fn(&Path) -> Result<S, String>,
    download: impl Fn(&Path, &str) -> Result<(), DownloadError>,
) -> Result<S, String> {
    let (file, current) = if path.is_file() {
        match read(path) {
            Ok(schema) => {
                let version = schema.as_ref().api_version.clone();
                (SchemaFile::Usable { version }, Some(schema))
            }
            Err(reason) => {
                let version = stored_api_version(path);
                (SchemaFile::Unusable { version, reason }, None)
            }
        }
    } else {
        (SchemaFile::Missing, None)
    };
    match schema_plan(dir, path, &file, pinned, may_download, now) {
        SchemaPlan::Read => current.map_or_else(|| read(path), Ok),
        SchemaPlan::Refuse(message) => Err(message),
        SchemaPlan::Download { version, latest } => {
            download_once(path, &version, latest, download).map_err(|error| {
                let which = match (pinned, &file) {
                    (Some(_), _) => format!("{version}, the version Cargo.toml pins"),
                    (None, SchemaFile::Missing) => format!(
                        "the latest Admin API version, {version}: Cargo.toml pins none, and there is no schema file yet"
                    ),
                    (None, _) => format!(
                        "{version}, the version of the schema file, again: this version of shopify_api cannot read it"
                    ),
                };
                download_failure(&error, path, &which, &version, now)
            })?;
            read(path)
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

    fn version_in(manifest: &str) -> Result<Option<String>, String> {
        config_in(manifest).map(|config| config.api_version)
    }

    #[test]
    fn the_version_is_read_from_the_package_metadata() {
        assert_eq!(version_in("[package]\nname = \"app\""), Ok(None));
        assert_eq!(
            version_in("[package.metadata.shopify_api]\napi-version = \"2026-07\""),
            Ok(Some("2026-07".to_string()))
        );
        assert_eq!(
            version_in("[package.metadata.shopify_api]\napi-version = \"unstable\""),
            Ok(Some("unstable".to_string()))
        );
        let old =
            version_in("[package.metadata.shopify_api]\napi-version = \"2025-01\"").unwrap_err();
        assert!(old.contains("is not a version shopify_api calls"), "{old}");
        let number = version_in("[package.metadata.shopify_api]\napi-version = 2026").unwrap_err();
        assert!(number.contains("must be a string"), "{number}");
        assert!(version_in("[package").is_err());
    }

    #[test]
    fn the_schema_file_is_read_from_the_package_metadata() {
        let schema = |manifest: &str| config_in(manifest).map(|config| config.schema);
        assert_eq!(schema("[package]\nname = \"app\""), Ok(None));
        assert_eq!(
            schema("[package.metadata.shopify_api]\nschema = \"schemas/admin.json\""),
            Ok(Some(PathBuf::from("schemas/admin.json")))
        );
        for wrong in ["schema = 1", "schema = \"\""] {
            let error = schema(&format!("[package.metadata.shopify_api]\n{wrong}")).unwrap_err();
            assert!(
                error.contains("must be the path of a schema file"),
                "{error}"
            );
        }
    }

    #[test]
    fn the_schema_snippet_reads_back_as_the_path() {
        // A backslash is not a `schema` value: `shopify-api` prints a note
        // instead of a snippet for such a name.
        for name in ["schemas/a\"b.json", "schemas/tab\t'q'.json", "admin.json"] {
            let snippet = schema_snippet("2026-07", Path::new(name));
            let config = config_in(&format!("[package]\nname = \"app\"\n\n{snippet}\n"));
            assert_eq!(
                config,
                Ok(PackageConfig {
                    api_version: Some("2026-07".to_string()),
                    schema: Some(PathBuf::from(name)),
                }),
                "{snippet}"
            );
        }
        let nested: PathBuf = ["schemas", "admin.json"].iter().collect();
        assert!(schema_snippet("2026-07", &nested).ends_with("schema = \"schemas/admin.json\""));
    }

    #[test]
    fn the_schema_file_must_be_inside_the_package() {
        let schema = |path: &str| {
            config_in(&format!(
                "[package.metadata.shopify_api]\nschema = {}",
                toml::Value::from(path)
            ))
            .map(|config| config.schema)
        };
        let absolute = std::env::temp_dir().join("admin.schema.json");
        for outside in [
            absolute.to_str().unwrap(),
            "/admin.schema.json",
            "../x.json",
            "a/../../x.json",
            "./..",
            ".",
            "a/..",
        ] {
            let error = schema(outside).unwrap_err();
            assert!(
                error.contains("must name a file inside the package"),
                "{error}"
            );
            assert!(
                error.contains("relative path with `/` separators and no `:` or `\\`"),
                "{error}"
            );
            assert!(error.contains("`cargo package` ships"), "{error}");
        }
        // Not portable, so refused on every system: a drive on Windows, even
        // in the middle of the path, and a separator there.
        for outside in [
            "./D:/schema.json",
            "schemas/D:/schema.json",
            "a/../D:/schema.json",
            "./D:../schema.json",
            "C:x.json",
            "a\\b.json",
            "schemas\\admin.json",
            "a:b/x.json",
        ] {
            let error = schema(outside).unwrap_err();
            assert!(error.contains("no `:` or `\\`"), "{outside}: {error}");
        }
        #[cfg(windows)]
        for outside in [
            "C:\\x.json",
            "C:x.json",
            "\\\\server\\share\\x.json",
            "\\x.json",
        ] {
            assert!(schema(outside).is_err(), "{outside}");
        }
        for (inside, normal) in [
            ("a/../x.json", "x.json"),
            ("./graphql/x.json", "graphql/x.json"),
            ("schemas/../schemas/admin.json", "schemas/admin.json"),
            ("schemas//admin.json", "schemas/admin.json"),
            ("schemas/admin.json/", "schemas/admin.json"),
        ] {
            assert_eq!(schema(inside), Ok(Some(PathBuf::from(normal))), "{inside}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_inside_the_package_may_point_outside_it() {
        let project = Project::new("link");
        let outside = Project::new("link-target");
        let target = outside.write("admin.schema.json", "{}");
        std::fs::create_dir_all(project.0.join("schemas")).unwrap();
        std::os::unix::fs::symlink(&target, project.0.join("schemas/admin.json")).unwrap();
        project.write(
            "Cargo.toml",
            "[package]\nname = \"app\"\n\n[package.metadata.shopify_api]\nschema = \"schemas/admin.json\"\n",
        );
        assert_eq!(
            schema_source(Some(&project.0), None),
            Ok(SchemaSource::Project {
                dir: project.0.clone(),
                path: project.0.join("schemas/admin.json"),
                pinned: None,
            })
        );
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
    fn the_manifest_names_another_file_relative_to_it() {
        let project = Project::new("key");
        project.write(
            "Cargo.toml",
            "[package]\nname = \"app\"\n\n[package.metadata.shopify_api]\n\
             api-version = \"2026-07\"\nschema = \"schemas/admin-2026-07.json\"\n",
        );
        // The file need not exist: missing, it is downloaded or refused as
        // the default one is.
        let expected = SchemaSource::Project {
            dir: project.0.clone(),
            path: project.0.join("schemas/admin-2026-07.json"),
            pinned: Some("2026-07".to_string()),
        };
        assert_eq!(schema_source(Some(&project.0), None), Ok(expected.clone()));
        // An empty test variable is no variable.
        assert_eq!(
            schema_source(Some(&project.0), Some(OsStr::new(""))),
            Ok(expected)
        );
    }

    #[test]
    fn the_test_variable_names_a_file_for_every_package() {
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
                "SHOPIFY_API_INTERNAL_TEST_SCHEMA names `{}`, which is not a file",
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

    fn path(dir: &Path) -> PathBuf {
        dir.join(PROJECT_SCHEMA_PATH)
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
                schema_plan(
                    dir,
                    &path(dir),
                    &usable("2026-07"),
                    None,
                    primary,
                    years_later
                ),
                SchemaPlan::Read
            );
        }
        // No file and no pin: the first build of the crate takes the latest.
        assert_eq!(
            schema_plan(
                dir,
                &path(dir),
                &SchemaFile::Missing,
                None,
                true,
                day(2027, 2, 3)
            ),
            download("2027-01", true)
        );
    }

    #[test]
    fn the_primary_package_downloads_its_pinned_version() {
        let dir = Path::new("/crate");
        let now = day(2026, 10, 22);
        assert_eq!(
            schema_plan(
                dir,
                &path(dir),
                &usable("2026-07"),
                Some("2026-07"),
                true,
                now
            ),
            SchemaPlan::Read
        );
        assert_eq!(
            schema_plan(
                dir,
                &path(dir),
                &usable("2026-07"),
                Some("2026-10"),
                true,
                now
            ),
            download("2026-10", false)
        );
        assert_eq!(
            schema_plan(
                dir,
                &path(dir),
                &SchemaFile::Missing,
                Some("2026-04"),
                true,
                now
            ),
            download("2026-04", false)
        );
        // A file of an older format is downloaded again, for the same version.
        assert_eq!(
            schema_plan(
                dir,
                &path(dir),
                &old_format(Some("2026-07")),
                None,
                true,
                now
            ),
            download("2026-07", false)
        );
        assert_eq!(
            schema_plan(
                dir,
                &path(dir),
                &old_format(Some("2026-07")),
                Some("2026-10"),
                true,
                now
            ),
            download("2026-10", false)
        );
        assert_eq!(
            schema_plan(dir, &path(dir), &old_format(None), None, true, now),
            download("2026-10", true)
        );
    }

    #[test]
    fn a_package_is_local_unless_packaged_vendored_or_under_cargo_home() {
        let home = Project::new("cargo-home");
        let checkout = home.write("git/checkouts/lib-1a2b/0c9d/Cargo.toml", "");
        let checkout = checkout.parent().unwrap();
        assert!(!is_local_package(checkout, Some(&home.0)));
        // Without a `CARGO_HOME`, only the files tell.
        assert!(is_local_package(checkout, None));

        let workspace = Project::new("local");
        workspace.manifest(None);
        assert!(is_local_package(&workspace.0, Some(&home.0)));
        assert!(is_local_package(&workspace.0, None));
        // A directory whose name starts as `CARGO_HOME`'s is not under it.
        let sibling = PathBuf::from(format!("{}-sibling", home.0.display()));
        assert!(is_local_package(&sibling, Some(&home.0)));

        for marker in ["Cargo.toml.orig", ".cargo-checksum.json"] {
            let packaged = Project::new(&format!("packaged{marker}"));
            packaged.manifest(None);
            packaged.write(marker, "");
            assert!(!is_local_package(&packaged.0, Some(&home.0)), "{marker}");
            assert!(!is_local_package(&packaged.0, None), "{marker}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_cargo_home_through_a_symbolic_link_is_still_cargo_home() {
        let real = Project::new("real-cargo-home");
        let registry = real.write("registry/src/index/lib-1.0.0/Cargo.toml", "");
        let linked = std::env::temp_dir().join(format!(
            "shopify-api-graphql-linked-home-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&linked);
        std::os::unix::fs::symlink(&real.0, &linked).unwrap();
        assert!(!is_local_package(registry.parent().unwrap(), Some(&linked)));
        let _ = std::fs::remove_file(&linked);
    }

    #[test]
    fn a_package_in_a_cargo_checkout_is_not_local() {
        let home = Project::new("marker-home");
        // `.cargo-ok` in the package directory itself, as in a registry source.
        let source = Project::new("marker-source");
        source.manifest(None);
        source.write(".cargo-ok", "{\"v\":1}");
        assert!(!is_local_package(&source.0, Some(&home.0)));
        assert!(!is_local_package(&source.0, None));

        // A package nested in a git checkout, `.cargo-ok` at its root.
        let checkout = Project::new("marker-checkout");
        checkout.write(".cargo-ok", "");
        let nested = checkout.write("crates/lib/Cargo.toml", "");
        let nested = nested.parent().unwrap();
        assert!(!is_local_package(nested, Some(&home.0)));
        assert!(!is_local_package(nested, None));
    }

    /// `$CARGO_HOME/<linked>` links to a directory outside `$CARGO_HOME`.
    #[cfg(unix)]
    fn linked_cargo_home(name: &str, linked: &str) -> (Project, Project) {
        let home = Project::new(&format!("{name}-home"));
        let store = Project::new(&format!("{name}-store"));
        let link = home.0.join(linked);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&store.0, &link).unwrap();
        (home, store)
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_to_the_git_directory_keeps_its_packages_not_local() {
        for (name, linked, below) in [
            ("linked-git", "git", "checkouts/lib-1a2b/0c9d"),
            ("linked-checkouts", "git/checkouts", "lib-1a2b/0c9d"),
        ] {
            let (home, store) = linked_cargo_home(name, linked);
            let package = home.0.join(linked).join(below).join("crates/lib");
            store.write(&format!("{below}/crates/lib/Cargo.toml"), "");
            // Without a marker, the path under `$CARGO_HOME` tells.
            assert!(!is_local_package(&package, Some(&home.0)), "{name}");
            // With its symbolic links resolved, the path is outside
            // `$CARGO_HOME`: the `.cargo-ok` at the checkout root tells.
            let resolved = package.canonicalize().unwrap();
            assert!(!resolved.starts_with(home.0.canonicalize().unwrap()));
            store.write(&format!("{below}/.cargo-ok"), "");
            assert!(!is_local_package(&package, Some(&home.0)), "{name}");
            assert!(!is_local_package(&resolved, Some(&home.0)), "{name}");
        }
    }

    #[test]
    fn only_a_local_primary_package_downloads() {
        let home = Project::new("download-home");
        let local = Project::new("download-local");
        local.manifest(None);
        assert!(may_download(&local.0, Some(&home.0), true));
        assert!(!may_download(&local.0, Some(&home.0), false));
        // Packaged: never, even when primary, as in the verification of
        // `cargo package`.
        local.write("Cargo.toml.orig", "");
        assert!(!may_download(&local.0, Some(&home.0), true));
    }

    #[test]
    fn a_dependency_never_downloads() {
        let dir = Path::new("/registry/deplib-1.0.0");
        let now = day(2026, 10, 22);
        let refusal = |file: &SchemaFile, pinned: Option<&str>| match schema_plan(
            dir,
            &path(dir),
            file,
            pinned,
            false,
            now,
        ) {
            SchemaPlan::Refuse(message) => {
                let fix = "Run `shopify-api schema download` in `/registry/deplib-1.0.0`";
                assert!(message.contains(fix), "{message}");
                message
            }
            other => panic!("{file:?}, {pinned:?}: {other:?}"),
        };
        assert_eq!(
            schema_plan(
                dir,
                &path(dir),
                &usable("2026-07"),
                Some("2026-07"),
                false,
                now
            ),
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
                let config = package_config(&project.0).unwrap();
                obtain_schema(
                    &project.0,
                    &config.schema_path(&project.0),
                    config.api_version.as_deref(),
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

        /// A package whose `Cargo.toml` names `schemas/admin.json`.
        fn with_key(name: &str, version: Option<&str>) -> Project {
            let project = Project::new(name);
            let pin = version.map_or(String::new(), |version| {
                format!("api-version = \"{version}\"\n")
            });
            project.write(
                "Cargo.toml",
                &format!(
                    "[package]\nname = \"app\"\n\n[package.metadata.shopify_api]\n\
                     {pin}schema = \"schemas/admin.json\"\n"
                ),
            );
            project
        }

        #[test]
        fn the_file_the_manifest_names_follows_the_rules_of_the_default_one() {
            let project = with_key("key-local", None);
            // The default file is not read when the key names another.
            project.write(PROJECT_SCHEMA_PATH, &schema("2026-04"));
            project.write("schemas/admin.json", &schema("2026-07"));
            let network = Network::new(true, &["2026-07", "2026-10"]);
            let now = day(2026, 10, 22);
            assert_eq!(
                network.obtain(&project, true, now).unwrap().api_version,
                "2026-07"
            );
            assert!(network.asked.borrow().is_empty());

            // Missing: a local primary package downloads it there.
            std::fs::remove_file(project.0.join("schemas/admin.json")).unwrap();
            assert_eq!(
                network.obtain(&project, true, now).unwrap().api_version,
                "2026-10"
            );
            assert_eq!(*network.asked.borrow(), ["2026-10"]);
            assert_eq!(
                read(&project.0.join("schemas/admin.json"))
                    .unwrap()
                    .api_version,
                "2026-10"
            );
            assert_eq!(
                read(&project.0.join(PROJECT_SCHEMA_PATH))
                    .unwrap()
                    .api_version,
                "2026-04"
            );
        }

        #[test]
        fn a_dependency_uses_the_file_its_manifest_names_and_never_downloads_it() {
            let now = day(2026, 10, 22);
            let network = Network::new(true, &["2026-04", "2026-07", "2026-10"]);
            let project = with_key("key-dependency", Some("2026-07"));
            project.write("schemas/admin.json", &schema("2026-07"));
            assert_eq!(
                network.obtain(&project, false, now).unwrap().api_version,
                "2026-07"
            );

            // Missing: an error that names the file and the command.
            std::fs::remove_file(project.0.join("schemas/admin.json")).unwrap();
            let error = network.obtain(&project, false, now).unwrap_err();
            let file = project.0.join("schemas/admin.json");
            assert!(
                error.starts_with(&format!("`{}` is missing", file.display())),
                "{error}"
            );
            assert!(
                error.contains(&format!(
                    "Run `shopify-api schema download` in `{}`",
                    project.0.display()
                )),
                "{error}"
            );

            // Of another version than the pinned one: an error too.
            project.write("schemas/admin.json", &schema("2026-04"));
            let error = network.obtain(&project, false, now).unwrap_err();
            assert!(error.contains("holds the Admin API 2026-04"), "{error}");
            assert!(network.asked.borrow().is_empty());
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
