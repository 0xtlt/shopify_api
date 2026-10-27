//! `shopify-api`: downloads the Shopify Admin schema of the version a crate's
//! `Cargo.toml` pins, else of the latest version, and checks `.graphql` files
//! against a schema.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use shopify_api::{ApiVersion, Auth, Shopify};
use std::sync::Arc;
use std::time::SystemTime;

use shopify_api_graphql::project::{
    DownloadError, PORTABLE_PATH, download_introspection, inside_package, package_config,
    previous_api_version, project_schema, schema_snippet, write_atomically,
};
use shopify_api_graphql::{Schema, SchemaSource, analyze, latest_api_version, schema_source};

#[derive(Parser)]
#[command(
    name = "shopify-api",
    version,
    about = "Shopify Admin GraphQL schemas for the graphql! macro of shopify_api"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Admin GraphQL schemas.
    Schema {
        #[command(subcommand)]
        command: SchemaCommand,
    },
    /// Checks `.graphql` files against the schema `graphql!` would use here.
    Check(CheckArgs),
}

#[derive(Subcommand)]
enum SchemaCommand {
    /// Downloads the Admin schema of the crate's version again, where
    /// `graphql!` reads it.
    ///
    /// The version is `api-version` in `[package.metadata.shopify_api]` of the
    /// nearest package, else the latest stable one: this moves a crate that
    /// pins none to the latest version. With the `schema-download` feature,
    /// `graphql!` downloads the file by itself when it is missing, in an older
    /// format, or of another version than the pinned one; this command
    /// writes or refreshes it, or downloads it through a shop with --shop.
    Download(DownloadArgs),
}

#[derive(Args)]
struct DownloadArgs {
    /// Where to write it. By default, where `graphql!` reads it for the
    /// nearest package: `schema` in `[package.metadata.shopify_api]` of its
    /// `Cargo.toml`, else `graphql/admin.schema.json` next to it
    /// (`graphql/admin.introspection.json` with --introspection).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Writes the raw introspection result, for other GraphQL tools, instead
    /// of the compact form `graphql!` reads.
    #[arg(long)]
    introspection: bool,
    /// Keeps only what these `.graphql` files use: the types they reach, and
    /// the fields they select. For a small schema to commit with a library.
    #[arg(long, num_args = 1.., value_name = "FILES", conflicts_with = "introspection")]
    only: Vec<PathBuf>,
    /// Downloads through this shop, with its token: the schema the token sees.
    #[arg(long, env = "SHOPIFY_SHOP")]
    shop: Option<String>,
    #[arg(long, env = "SHOPIFY_ACCESS_TOKEN", hide_env_values = true)]
    access_token: Option<String>,
    #[arg(long, env = "SHOPIFY_CLIENT_ID")]
    client_id: Option<String>,
    #[arg(long, env = "SHOPIFY_CLIENT_SECRET", hide_env_values = true)]
    client_secret: Option<String>,
}

#[derive(Args)]
struct CheckArgs {
    /// The `.graphql` files, one operation each.
    #[arg(required = true)]
    files: Vec<PathBuf>,
    /// The schema file; else the one `graphql!` would use for the nearest package.
    #[arg(long)]
    schema: Option<PathBuf>,
    /// Fails on a deprecated field, argument or value too.
    #[arg(long)]
    deny_deprecated: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let outcome = match cli.command {
        Command::Schema {
            command: SchemaCommand::Download(args),
        } => download(args).await,
        Command::Check(args) => check(args),
    };
    match outcome {
        Ok(code) => code,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

async fn download(args: DownloadArgs) -> Result<ExitCode, String> {
    let root = package_root().ok_or(
        "no Cargo.toml of a package in this directory or above: run it in the crate's directory",
    )?;
    let config = package_config(&root)?;
    let declared = config.api_version.clone();
    let latest = declared.is_none();
    let wanted = declared.unwrap_or_else(|| latest_api_version(SystemTime::now()));
    if latest {
        eprintln!(
            "{} declares no Admin API version: the latest is {wanted}.",
            root.join("Cargo.toml").display()
        );
    }
    let version = ApiVersion::new(&wanted).map_err(|error| error.to_string())?;
    let (version, introspection) = match &args.shop {
        None => public_introspection(version, latest)?,
        Some(shop) => (version, through_shop(&args, shop, version).await?),
    };

    let (out, project) = match &args.out {
        Some(out) => (out.clone(), false),
        None => {
            let file = if args.introspection {
                root.join("graphql/admin.introspection.json")
            } else {
                config.schema_path(&root)
            };
            (file, !args.introspection)
        }
    };
    let created = !out.exists();
    let (contents, types) = if args.introspection {
        let types = introspection
            .pointer("/data/__schema/types")
            .and_then(|types| types.as_array())
            .map_or(0, Vec::len);
        let pretty =
            serde_json::to_string_pretty(&introspection).map_err(|error| error.to_string())?;
        (format!("{pretty}\n"), types)
    } else {
        let mut schema = Schema::from_introspection(&introspection, &version.to_string())
            .map_err(|error| error.to_string())?;
        if !args.only.is_empty() {
            let texts = args
                .only
                .iter()
                .map(|file| {
                    std::fs::read_to_string(file)
                        .map_err(|error| format!("cannot read {}: {error}", file.display()))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let texts: Vec<&str> = texts.iter().map(String::as_str).collect();
            schema = schema
                .restricted_to(&texts)
                .map_err(|error| error.to_string())?;
        }
        (schema.to_json(), schema.types.len())
    };
    write_atomically(&out, &contents)?;
    eprintln!("Wrote {} ({types} types).", out.display());

    if project {
        if created {
            touch_sources(&root);
        }
        eprintln!(
            "`graphql!` checks the operations of this crate against the Admin API {version}, \
             and sends them to that version. Commit the file: offline builds need it."
        );
    } else if !args.introspection {
        match schema_key(&root, &out) {
            Some(named) => eprintln!(
                "For `graphql!` to use it, name it in {}:\n\n{}",
                root.join("Cargo.toml").display(),
                schema_snippet(&version.to_string(), &named)
            ),
            None => eprintln!(
                "`graphql!` cannot use it: `schema` in {} names a file inside the package, \
                 as {PORTABLE_PATH}, and `cargo package` ships only the package's files.",
                root.join("Cargo.toml").display()
            ),
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// The `schema` value that names `out`, a file just written, for the
/// package in `root`, if any: a path [`inside_package`] accepts, that leads
/// to that same file. `a/../x.json` is normalized to `x.json`, which is
/// another file when `a` links to a directory elsewhere.
fn schema_key(root: &Path, out: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let absolute = std::path::absolute(out).ok()?;
    let named = absolute.strip_prefix(root).ok()?;
    let value = named
        .components()
        .map(|component| match component {
            Component::Normal(name) => name.to_str(),
            Component::CurDir => Some("."),
            Component::ParentDir => Some(".."),
            Component::Prefix(_) | Component::RootDir => None,
        })
        .collect::<Option<Vec<_>>>()?
        .join("/");
    let normal = inside_package(&value).ok()?;
    let written = out.canonicalize().ok()?;
    let named = root.join(&normal).canonicalize().ok()?;
    (named == written).then_some(normal)
}

/// The introspection result for `version` from Shopify's public endpoint,
/// and its version: for the `latest` one, the one before when Shopify does
/// not have it yet, as on the first day of a quarter.
fn public_introspection(
    version: ApiVersion,
    latest: bool,
) -> Result<(ApiVersion, serde_json::Value), String> {
    let download = |version: ApiVersion| {
        eprintln!("Downloading the Admin schema {version} from Shopify's public endpoint…");
        download_introspection(&version.to_string()).map(|value| (version, value))
    };
    match download(version) {
        Err(error @ DownloadError::UnknownVersion { .. }) if latest => {
            let previous = previous_api_version(&version.to_string())
                .and_then(|previous| ApiVersion::new(&previous).ok())
                .ok_or_else(|| error.to_string())?;
            eprintln!("{error}\nTrying the version before, {previous}.");
            download(previous).map_err(|error| error.to_string())
        }
        outcome => outcome.map_err(|error| error.to_string()),
    }
}

/// The introspection result for `version` through `shop`, with the token of `args`.
async fn through_shop(
    args: &DownloadArgs,
    shop: &str,
    version: ApiVersion,
) -> Result<serde_json::Value, String> {
    let auth = match (&args.access_token, &args.client_id, &args.client_secret) {
        (Some(token), _, _) => Auth::access_token(token),
        (None, Some(id), Some(secret)) => Auth::client_credentials(id, secret),
        _ => {
            return Err(
                "--shop needs --access-token, or --client-id and --client-secret".to_string(),
            );
        }
    };
    let shopify = Shopify::builder(shop)
        .auth(auth)
        .api_version(version)
        .build()
        .map_err(|error| error.to_string())?;
    eprintln!(
        "Downloading the Admin schema {version} through {}…",
        shopify.shop()
    );
    shopify
        .download_admin_schema()
        .await
        .map_err(|error| error.to_string())
}

/// Cargo follows a schema file once `graphql!` read it, not before it
/// exists: touching the sources makes the next build read the new file.
fn touch_sources(package: &Path) {
    for source in ["src/main.rs", "src/lib.rs"] {
        if let Ok(file) = std::fs::File::options()
            .write(true)
            .open(package.join(source))
        {
            let _ = file.set_modified(std::time::SystemTime::now());
        }
    }
}

/// The directory of the nearest `Cargo.toml` that declares a package, from
/// the current directory up: where `graphql!` resolves its paths.
fn package_root() -> Option<PathBuf> {
    let mut directory = std::env::current_dir().ok()?;
    loop {
        let manifest = std::fs::read_to_string(directory.join("Cargo.toml"));
        if manifest.is_ok_and(|text| text.lines().any(|line| line.trim() == "[package]")) {
            return Some(directory);
        }
        if !directory.pop() {
            return None;
        }
    }
}

/// The schema `graphql!` would use for the nearest package, as it looks for it.
fn load_schema(path: Option<&Path>) -> Result<(Schema, String), String> {
    let read = |path: &Path| -> Result<Arc<Schema>, String> {
        let json = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        Schema::from_json(&json)
            .map(Arc::new)
            .map_err(|error| format!("{}: {error}", path.display()))
    };
    let path = match path {
        Some(path) => path.to_path_buf(),
        None => match schema_source(package_root().as_deref(), None, None)? {
            SchemaSource::File(path) => path,
            SchemaSource::Project { dir, path, pinned } => {
                let schema = project_schema(
                    &dir,
                    &path,
                    pinned.as_deref(),
                    true,
                    SystemTime::now(),
                    read,
                    |file, version| {
                        eprintln!(
                            "Downloading the Admin schema {version} into {}…",
                            file.display()
                        );
                    },
                )?;
                return Ok((Arc::unwrap_or_clone(schema), path.display().to_string()));
            }
        },
    };
    let schema = read(&path)?;
    Ok((Arc::unwrap_or_clone(schema), path.display().to_string()))
}

fn check(args: CheckArgs) -> Result<ExitCode, String> {
    let (schema, source) = load_schema(args.schema.as_deref())?;
    eprintln!(
        "Checking against the Admin API {} ({source}).",
        schema.api_version
    );
    let mut failed = false;
    for file in &args.files {
        let text = match std::fs::read_to_string(file) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("{}: cannot read: {error}", file.display());
                failed = true;
                continue;
            }
        };
        match analyze(&schema, &text) {
            Ok(operation) => {
                for deprecation in &operation.deprecations {
                    let (line, column) = deprecation.position;
                    let level = if args.deny_deprecated {
                        "error"
                    } else {
                        "warning"
                    };
                    eprintln!(
                        "{}:{line}:{column}: {level}: `{}` is deprecated: {}",
                        file.display(),
                        deprecation.coordinate,
                        deprecation.reason
                    );
                }
                failed |= args.deny_deprecated && !operation.deprecations.is_empty();
                let name = operation.name.as_deref().unwrap_or("(anonymous)");
                eprintln!("{}: ok, {:?} {name}", file.display(), operation.kind);
            }
            Err(error) => {
                failed = true;
                match error.position {
                    Some((line, column)) => {
                        eprintln!(
                            "{}:{line}:{column}: error: {}",
                            file.display(),
                            error.message
                        )
                    }
                    None => eprintln!("{}: error: {}", file.display(), error.message),
                }
            }
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty directory, canonical, removed when the test ends.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("shopify-api-cli-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir.canonicalize().unwrap())
        }

        fn write(&self, path: &str) -> PathBuf {
            let file = self.0.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, "{}").unwrap();
            file
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_key_names_the_file_written() {
        let package = Dir::new("key");
        let out = package.write("schemas/admin.json");
        assert_eq!(
            schema_key(&package.0, &out),
            Some(PathBuf::from("schemas/admin.json"))
        );
        package.write("x.json");
        assert_eq!(
            schema_key(&package.0, &package.0.join("schemas/../x.json")),
            Some(PathBuf::from("x.json"))
        );
        let outside = Dir::new("key-outside");
        assert_eq!(schema_key(&package.0, &outside.write("x.json")), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_name_that_is_not_a_portable_key_has_none() {
        let package = Dir::new("key-portable");
        for name in ["schemas/a\\b.json", "schemas/D:x.json"] {
            let out = package.write(name);
            assert_eq!(schema_key(&package.0, &out), None, "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_followed_by_dot_dot_names_another_file() {
        let package = Dir::new("key-link");
        let outside = Dir::new("key-link-outside");
        std::fs::create_dir_all(outside.0.join("dir")).unwrap();
        std::os::unix::fs::symlink(outside.0.join("dir"), package.0.join("a")).unwrap();
        // Written through the link, `a/../x.json` is `outside/x.json`.
        let out = package.0.join("a/../x.json");
        std::fs::write(&out, "{}").unwrap();
        assert!(outside.0.join("x.json").is_file());
        assert_eq!(schema_key(&package.0, &out), None);
        // Even when the package has an `x.json` of its own.
        package.write("x.json");
        assert_eq!(schema_key(&package.0, &out), None);
        // A link inside the package to a file elsewhere is followed by both.
        std::os::unix::fs::symlink(outside.0.join("x.json"), package.0.join("linked.json"))
            .unwrap();
        assert_eq!(
            schema_key(&package.0, &package.0.join("linked.json")),
            Some(PathBuf::from("linked.json"))
        );
    }
}
