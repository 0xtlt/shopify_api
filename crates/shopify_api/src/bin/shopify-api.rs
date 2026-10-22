//! `shopify-api`: downloads Shopify Admin schemas for `graphql!`, and checks
//! `.graphql` files against them.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};
use shopify_api::{ApiVersion, Auth, Shopify};
use shopify_api_graphql::{PROJECT_SCHEMA_PATH, SCHEMA_ENV, Schema, analyze};

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
    /// Downloads the Admin schema of a version, where `graphql!` reads it.
    ///
    /// From Shopify's public endpoint by default; through a shop with --shop.
    Download(DownloadArgs),
}

#[derive(Debug, Args)]
struct DownloadArgs {
    /// The Admin API version.
    #[arg(long, default_value_t = ApiVersion::DEFAULT.to_string())]
    api_version: String,
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
    let version = ApiVersion::new(&args.api_version).map_err(|error| error.to_string())?;
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
            let root = package_root().ok_or(
                "no Cargo.toml of a package in this directory or above: run it in the crate's directory, or pass --out",
            )?;
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
    if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    std::fs::write(&out, contents)
        .map_err(|error| format!("cannot write {}: {error}", out.display()))?;
    eprintln!("Wrote {} ({types} types).", out.display());

    if project {
        if created && let Some(root) = out.parent().and_then(Path::parent) {
            touch_sources(root);
        }
        eprintln!(
            "`graphql!` now checks the operations of this crate against the Admin API {version}, \
             and sends them to that version. Commit the file: builds need it."
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
    let root = package_root();
    let resolve = |path: PathBuf| match &root {
        Some(root) if path.is_relative() => root.join(path),
        _ => path,
    };
    let named = std::env::var_os(SCHEMA_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .map(resolve);
    let project = root
        .as_ref()
        .map(|root| root.join(PROJECT_SCHEMA_PATH))
        .filter(|path| path.is_file());
    let path = path.map(Path::to_path_buf).or(named).or(project);
    match path {
        Some(path) => {
            let json = std::fs::read_to_string(&path)
                .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
            let schema =
                Schema::from_json(&json).map_err(|error| format!("{}: {error}", path.display()))?;
            Ok((schema, path.display().to_string()))
        }
        None => Ok((Schema::bundled().clone(), "the bundled schema".to_string())),
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
