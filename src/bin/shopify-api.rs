use std::{fs::File, path::PathBuf};

use clap::{Parser, Subcommand};
use shopify_api::{ApiVersion, Shopify, ShopifyAPIError, ShopifyAuth, ShopifyConfig};

#[derive(Debug, Parser)]
#[command(name = "shopify-api")]
#[command(about = "Shopify API helper commands")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Schema {
        #[command(subcommand)]
        command: SchemaCommands,
    },
}

#[derive(Debug, Subcommand)]
enum SchemaCommands {
    Download {
        #[arg(long)]
        shop: String,
        #[arg(long)]
        access_token: Option<String>,
        #[arg(long)]
        client_id: Option<String>,
        #[arg(long)]
        client_secret: Option<String>,
        #[arg(long, default_value = shopify_api::DEFAULT_API_VERSION)]
        api_version: String,
        #[arg(long)]
        out: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<(), ShopifyAPIError> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Schema {
            command:
                SchemaCommands::Download {
                    shop,
                    access_token,
                    client_id,
                    client_secret,
                    api_version,
                    out,
                },
        } => {
            let auth = match (access_token, client_id, client_secret) {
                (Some(token), None, None) => ShopifyAuth::AccessToken(token),
                (None, Some(client_id), Some(client_secret)) => {
                    ShopifyAuth::client_credentials(client_id, client_secret)
                }
                _ => {
                    return Err(ShopifyAPIError::Authentication(
                        "provide either --access-token or both --client-id and --client-secret"
                            .to_string(),
                    ));
                }
            };

            let config = ShopifyConfig {
                api_version: ApiVersion::new(api_version)?,
                ..ShopifyConfig::default()
            };
            let shopify = Shopify::new(shop, auth, config)?;
            let schema = shopify.download_admin_schema().await?;
            let file = File::create(out).map_err(|err| ShopifyAPIError::Other(err.to_string()))?;
            serde_json::to_writer_pretty(file, &schema)?;
        }
    }

    Ok(())
}
