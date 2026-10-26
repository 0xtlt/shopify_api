//! `shopify-api`: downloads the Shopify Admin schema of a crate's API version
//! (the one its `Cargo.toml` declares, else the latest), and checks
//! `.graphql` files against it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use shopify_api::{ApiVersion, Auth, Shopify};
use std::sync::Arc;
use std::time::SystemTime;

use shopify_api_graphql::project::{
    API_VERSION_KEY, METADATA_TABLE, SCHEMA_KEY, package_config, previous_api_version,
    project_schema, write_atomically,
};
use shopify_api_graphql::{Schema, SchemaSource, analyze, latest_api_version, schema_source};

#[derive(Debug, Parser)]
#[command(
    name = "shopify-api",
    version,
    about = "Shopify Admin GraphQL schemas for the graphql! macro of shopify_api"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Admin GraphQL schemas.
    Schema {
        #[command(subcommand)]
        command: SchemaCommand,
    },
    /// Checks `.graphql` files against the schema `graphql!` would use here.
    Check(CheckArgs),
}

#[derive(Debug, Subcommand)]
enum SchemaCommand {
    /// Downloads the Admin schema of the crate's version again, where
    /// `graphql!` reads it.
    ///
    /// The version is `api-version` in `[package.metadata.shopify_api]` of the
    /// nearest package, else the latest stable one: this moves a crate that
    /// pins none to the latest version. `graphql!` downloads the file by itself
    /// only when it is missing, in an older format, or of another version than
    /// the pinned one; this command refreshes it, or downloads it through a
    /// shop with --shop.
    Download(DownloadArgs),
}

/// `Debug` hides the access token and the client secret.
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

impl std::fmt::Debug for DownloadArgs {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let hidden = |secret: &Option<String>| secret.as_ref().map(|_| "[redacted]");
        formatter
            .debug_struct("DownloadArgs")
            .field("out", &self.out)
            .field("introspection", &self.introspection)
            .field("only", &self.only)
            .field("shop", &self.shop)
            .field("access_token", &hidden(&self.access_token))
            .field("client_id", &self.client_id)
            .field("client_secret", &hidden(&self.client_secret))
            .finish()
    }
}

#[derive(Debug, Args)]
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
    let mut version = ApiVersion::new(&wanted).map_err(|error| error.to_string())?;
    let introspection = match fetch(&args, version).await {
        Ok(introspection) => introspection,
        // A new version may not be out on its first day.
        Err(error) if latest && args.shop.is_none() => {
            let Some(previous) = previous_api_version(&wanted) else {
                return Err(error);
            };
            eprintln!("{error}\nTrying the version before, {previous}.");
            version = ApiVersion::new(&previous).map_err(|error| error.to_string())?;
            fetch(&args, version).await?
        }
        Err(error) => return Err(error),
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
        let absolute = std::path::absolute(&out).unwrap_or_else(|_| out.clone());
        let named = absolute.strip_prefix(&root).unwrap_or(&absolute);
        eprintln!(
            "For `graphql!` to use it, name it in {}:\n\n\
             [package.metadata.{METADATA_TABLE}]\n{API_VERSION_KEY} = \"{version}\"\n{SCHEMA_KEY} = \"{}\"",
            root.join("Cargo.toml").display(),
            named.display()
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// The introspection result for `version`, from Shopify's public endpoint or
/// through the shop of `args`.
async fn fetch(args: &DownloadArgs, version: ApiVersion) -> Result<serde_json::Value, String> {
    let Some(shop) = &args.shop else {
        eprintln!("Downloading the Admin schema {version} from Shopify's public endpoint…");
        return shopify_api::schema::download_public_admin_schema(version)
            .await
            .map_err(|error| error.to_string());
    };
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
        None => match schema_source(package_root().as_deref(), None)? {
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
