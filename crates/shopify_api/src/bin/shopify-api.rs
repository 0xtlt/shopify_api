//! `shopify-api`: downloads the Shopify Admin schema of the version a crate
//! declares in its `Cargo.toml`, and checks `.graphql` files against it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use shopify_api::{ApiVersion, Auth, Shopify};
use shopify_api_graphql::project::{
    configured_api_version, download_schema_file, metadata_snippet, write_atomically,
};
use shopify_api_graphql::{
    BUNDLED_API_VERSION, PROJECT_SCHEMA_PATH, SCHEMA_ENV, Schema, SchemaSource, analyze,
    schema_source,
};

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
    /// Downloads the Admin schema of the version in `Cargo.toml` again, where
    /// `graphql!` reads it.
    ///
    /// The version is `api-version` in `[package.metadata.shopify_api]` of the
    /// nearest package. `graphql!` downloads it by itself when the file is
    /// missing or holds another version: this command refreshes it, or
    /// downloads it through a shop with --shop.
    Download(DownloadArgs),
}

#[derive(Debug, Args)]
struct DownloadArgs {
    /// Where to write it. By default, `graphql/admin.schema.json` next to the
    /// nearest `Cargo.toml` of a package, where `graphql!` reads it
    /// (`graphql/admin.introspection.json` with --introspection).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Writes the raw introspection result, for other GraphQL tools, instead
    /// of the compact form `graphql!` reads.
    #[arg(long)]
    introspection: bool,
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
    let declared = configured_api_version(&root)?.ok_or_else(|| {
        format!(
            "{} declares no Admin API version. Add:\n\n{}",
            root.join("Cargo.toml").display(),
            metadata_snippet(BUNDLED_API_VERSION)
        )
    })?;
    let version = ApiVersion::new(&declared).map_err(|error| error.to_string())?;
    let introspection = match &args.shop {
        None => {
            eprintln!("Downloading the Admin schema {version} from Shopify's public endpoint…");
            shopify_api::schema::download_public_admin_schema(version)
                .await
                .map_err(|error| error.to_string())?
        }
        Some(shop) => {
            let auth = match (&args.access_token, &args.client_id, &args.client_secret) {
                (Some(token), _, _) => Auth::access_token(token),
                (None, Some(id), Some(secret)) => Auth::client_credentials(id, secret),
                _ => {
                    return Err(
                        "--shop needs --access-token, or --client-id and --client-secret"
                            .to_string(),
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
                .map_err(|error| error.to_string())?
        }
    };

    let (out, project) = match &args.out {
        Some(out) => (out.clone(), false),
        None => {
            let file = if args.introspection {
                "graphql/admin.introspection.json"
            } else {
                PROJECT_SCHEMA_PATH
            };
            (root.join(file), !args.introspection)
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
        let schema = Schema::from_introspection(&introspection, &version.to_string())
            .map_err(|error| error.to_string())?;
        (schema.to_json(), schema.types.len())
    };
    write_atomically(&out, &contents)?;
    eprintln!("Wrote {} ({types} types).", out.display());

    if project {
        if created && let Some(root) = out.parent().and_then(Path::parent) {
            touch_sources(root);
        }
        eprintln!(
            "`graphql!` checks the operations of this crate against the Admin API {version}, \
             and sends them to that version. Commit the file: offline builds need it."
        );
    } else if !args.introspection {
        eprintln!(
            "Set {SCHEMA_ENV}={} for `graphql!` to use it.",
            out.display()
        );
    }
    Ok(ExitCode::SUCCESS)
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
    let read = |path: &Path| -> Result<Schema, String> {
        let json = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        Schema::from_json(&json).map_err(|error| format!("{}: {error}", path.display()))
    };
    if let Some(path) = path {
        return Ok((read(path)?, path.display().to_string()));
    }
    let named = std::env::var_os(SCHEMA_ENV);
    match schema_source(package_root().as_deref(), named.as_deref())? {
        SchemaSource::Bundled => Ok((Schema::bundled().clone(), "the bundled schema".to_string())),
        SchemaSource::File(path) => Ok((read(&path)?, path.display().to_string())),
        SchemaSource::Declared { path, version } => {
            let schema = match read(&path) {
                Ok(schema) if schema.api_version == version => schema,
                _ => {
                    eprintln!(
                        "Downloading the Admin schema {version} into {}…",
                        path.display()
                    );
                    download_schema_file(&path, &version)?
                }
            };
            Ok((schema, path.display().to_string()))
        }
    }
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
