use std::{future::Future, pin::Pin};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{Shopify, ShopifyAPIError};

pub(crate) const ADMIN_ACCESS_TOKEN_HEADER: &str = "X-Shopify-Access-Token";
pub(crate) const STOREFRONT_PUBLIC_ACCESS_TOKEN_HEADER: &str = "X-Shopify-Storefront-Access-Token";
pub(crate) const STOREFRONT_PRIVATE_ACCESS_TOKEN_HEADER: &str = "Shopify-Storefront-Private-Token";

pub type TokenStoreFuture<'a> =
    Pin<Box<dyn Future<Output = Result<(), ShopifyAPIError>> + Send + 'a>>;

pub trait TokenStore: Send + Sync {
    fn save_token<'a>(&'a self, shop: &'a str, token: TokenData) -> TokenStoreFuture<'a>;
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct TokenData {
    pub access_token: String,
    pub scope: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub refresh_token: Option<String>,
    pub refresh_token_expires_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for TokenData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenData")
            .field("access_token", &"<redacted>")
            .field("scope", &self.scope)
            .field("expires_at", &self.expires_at)
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .field("refresh_token_expires_at", &self.refresh_token_expires_at)
            .finish()
    }
}

impl TokenData {
    pub fn never_expiring(access_token: impl Into<String>) -> Self {
        Self {
            access_token: access_token.into(),
            scope: None,
            expires_at: None,
            refresh_token: None,
            refresh_token_expires_at: None,
        }
    }

    pub fn expires_within(&self, leeway: chrono::Duration) -> bool {
        self.expires_at
            .map(|expires_at| expires_at <= Utc::now() + leeway)
            .unwrap_or(false)
    }
}

#[derive(Clone, Eq, PartialEq)]
pub enum ShopifyAuth {
    AccessToken(String),
    ClientCredentials {
        client_id: String,
        client_secret: String,
        current_token: Option<TokenData>,
    },
    ExpiringOfflineToken {
        client_id: String,
        client_secret: String,
        token: TokenData,
    },
}

/// Authentication accepted by Shopify's Storefront GraphQL API.
#[derive(Clone, Eq, PartialEq)]
pub enum StorefrontAuth {
    /// Send no access token. Shopify limits the fields available to tokenless requests.
    Tokenless,
    /// Authenticate with a public Storefront access token.
    PublicAccessToken(String),
    /// Authenticate with a private Storefront access token for server-side requests.
    PrivateAccessToken(String),
}

impl std::fmt::Debug for StorefrontAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorefrontAuth::Tokenless => f.write_str("Tokenless"),
            StorefrontAuth::PublicAccessToken(_) => f
                .debug_tuple("PublicAccessToken")
                .field(&"<redacted>")
                .finish(),
            StorefrontAuth::PrivateAccessToken(_) => f
                .debug_tuple("PrivateAccessToken")
                .field(&"<redacted>")
                .finish(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ApiAuth {
    Admin(ShopifyAuth),
    Storefront(StorefrontAuth),
}

impl std::fmt::Debug for ShopifyAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShopifyAuth::AccessToken(_) => {
                f.debug_tuple("AccessToken").field(&"<redacted>").finish()
            }
            ShopifyAuth::ClientCredentials { current_token, .. } => f
                .debug_struct("ClientCredentials")
                .field("client_id", &"<redacted>")
                .field("client_secret", &"<redacted>")
                .field("current_token", current_token)
                .finish(),
            ShopifyAuth::ExpiringOfflineToken { token, .. } => f
                .debug_struct("ExpiringOfflineToken")
                .field("client_id", &"<redacted>")
                .field("client_secret", &"<redacted>")
                .field("token", token)
                .finish(),
        }
    }
}

impl ShopifyAuth {
    pub fn client_credentials(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
    ) -> Self {
        Self::ClientCredentials {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            current_token: None,
        }
    }

    pub fn expiring_offline_token(
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        token: TokenData,
    ) -> Self {
        Self::ExpiringOfflineToken {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            token,
        }
    }
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    scope: Option<String>,
    expires_in: Option<i64>,
    refresh_token: Option<String>,
    refresh_token_expires_in: Option<i64>,
}

impl TokenResponse {
    fn into_token_data(self) -> TokenData {
        TokenData {
            access_token: self.access_token,
            scope: self.scope,
            expires_at: self
                .expires_in
                .map(|seconds| Utc::now() + chrono::Duration::seconds(seconds)),
            refresh_token: self.refresh_token,
            refresh_token_expires_at: self
                .refresh_token_expires_in
                .map(|seconds| Utc::now() + chrono::Duration::seconds(seconds)),
        }
    }
}

impl Shopify {
    pub async fn access_token(&self) -> Result<String, ShopifyAPIError> {
        let auth = self
            .auth
            .lock()
            .map_err(|_| ShopifyAPIError::Authentication("auth lock poisoned".to_string()))?
            .clone();

        match auth {
            ApiAuth::Admin(auth) => self.admin_access_token(auth).await,
            ApiAuth::Storefront(StorefrontAuth::PublicAccessToken(token))
            | ApiAuth::Storefront(StorefrontAuth::PrivateAccessToken(token)) => Ok(token),
            ApiAuth::Storefront(StorefrontAuth::Tokenless) => Err(ShopifyAPIError::Authentication(
                "tokenless Storefront API client has no access token".to_string(),
            )),
        }
    }

    pub(crate) async fn graphql_auth_header(
        &self,
    ) -> Result<Option<(&'static str, String)>, ShopifyAPIError> {
        let auth = self
            .auth
            .lock()
            .map_err(|_| ShopifyAPIError::Authentication("auth lock poisoned".to_string()))?
            .clone();

        match auth {
            ApiAuth::Admin(auth) => Ok(Some((
                ADMIN_ACCESS_TOKEN_HEADER,
                self.admin_access_token(auth).await?,
            ))),
            ApiAuth::Storefront(StorefrontAuth::Tokenless) => Ok(None),
            ApiAuth::Storefront(StorefrontAuth::PublicAccessToken(token)) => {
                Ok(Some((STOREFRONT_PUBLIC_ACCESS_TOKEN_HEADER, token)))
            }
            ApiAuth::Storefront(StorefrontAuth::PrivateAccessToken(token)) => {
                Ok(Some((STOREFRONT_PRIVATE_ACCESS_TOKEN_HEADER, token)))
            }
        }
    }

    async fn admin_access_token(&self, auth: ShopifyAuth) -> Result<String, ShopifyAPIError> {
        match auth {
            ShopifyAuth::AccessToken(token) => Ok(token),
            ShopifyAuth::ClientCredentials {
                client_id,
                client_secret,
                current_token,
            } => {
                if let Some(token) = current_token {
                    if !token.expires_within(self.token_refresh_leeway) {
                        return Ok(token.access_token);
                    }
                }

                let token = self
                    .request_client_credentials_token(&client_id, &client_secret)
                    .await?;
                self.persist_and_replace_auth(
                    ShopifyAuth::ClientCredentials {
                        client_id,
                        client_secret,
                        current_token: Some(token.clone()),
                    },
                    token.clone(),
                )
                .await?;
                Ok(token.access_token)
            }
            ShopifyAuth::ExpiringOfflineToken {
                client_id,
                client_secret,
                token,
            } => {
                if !token.expires_within(self.token_refresh_leeway) {
                    return Ok(token.access_token);
                }

                let refresh_token = token.refresh_token.as_deref().ok_or_else(|| {
                    ShopifyAPIError::Authentication(
                        "expiring offline token is missing refresh_token".to_string(),
                    )
                })?;
                let refreshed = self
                    .refresh_token_with_credentials(&client_id, &client_secret, refresh_token)
                    .await?;
                self.persist_and_replace_auth(
                    ShopifyAuth::ExpiringOfflineToken {
                        client_id,
                        client_secret,
                        token: refreshed.clone(),
                    },
                    refreshed.clone(),
                )
                .await?;
                Ok(refreshed.access_token)
            }
        }
    }

    async fn persist_and_replace_auth(
        &self,
        auth: ShopifyAuth,
        token: TokenData,
    ) -> Result<(), ShopifyAPIError> {
        self.replace_auth(auth)?;
        if let Some(store) = &self.token_store {
            store.save_token(self.shop_domain(), token).await?;
        }
        Ok(())
    }

    pub async fn request_client_credentials_token(
        &self,
        client_id: &str,
        client_secret: &str,
    ) -> Result<TokenData, ShopifyAPIError> {
        self.ensure_admin_api("client credentials token exchange")?;
        let response = self
            .client()
            .post(self.token_url())
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", client_id),
                ("client_secret", client_secret),
            ])
            .send()
            .await?
            .error_for_status()?
            .json::<TokenResponse>()
            .await?;

        Ok(response.into_token_data())
    }

    pub async fn refresh_token_with_credentials(
        &self,
        client_id: &str,
        client_secret: &str,
        refresh_token: &str,
    ) -> Result<TokenData, ShopifyAPIError> {
        self.ensure_admin_api("offline token refresh")?;
        let response = self
            .client()
            .post(self.token_url())
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("refresh_token", refresh_token),
            ])
            .send()
            .await?
            .error_for_status()?
            .json::<TokenResponse>()
            .await?;

        Ok(response.into_token_data())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_expiration_uses_leeway() {
        let token = TokenData {
            access_token: "token".to_string(),
            scope: None,
            expires_at: Some(Utc::now() + chrono::Duration::minutes(1)),
            refresh_token: None,
            refresh_token_expires_at: None,
        };

        assert!(token.expires_within(chrono::Duration::minutes(5)));
        assert!(!token.expires_within(chrono::Duration::seconds(1)));
    }

    #[test]
    fn static_token_never_expires() {
        let token = TokenData::never_expiring("token");

        assert!(!token.expires_within(chrono::Duration::days(365)));
    }

    #[tokio::test]
    async fn admin_graphql_requests_use_the_admin_header() {
        let shopify = Shopify::new(
            "example",
            ShopifyAuth::AccessToken("admin-token".to_string()),
            crate::ShopifyConfig::default(),
        )
        .unwrap();

        assert_eq!(
            shopify.graphql_auth_header().await.unwrap(),
            Some((ADMIN_ACCESS_TOKEN_HEADER, "admin-token".to_string()))
        );
    }

    #[tokio::test]
    async fn storefront_graphql_requests_use_the_selected_authentication() {
        let public = Shopify::new_storefront(
            "example",
            StorefrontAuth::PublicAccessToken("public-token".to_string()),
            crate::ShopifyConfig::default(),
        )
        .unwrap();
        let private = Shopify::new_storefront(
            "example",
            StorefrontAuth::PrivateAccessToken("private-token".to_string()),
            crate::ShopifyConfig::default(),
        )
        .unwrap();
        let tokenless = Shopify::new_storefront(
            "example",
            StorefrontAuth::Tokenless,
            crate::ShopifyConfig::default(),
        )
        .unwrap();

        assert_eq!(
            public.graphql_auth_header().await.unwrap(),
            Some((
                STOREFRONT_PUBLIC_ACCESS_TOKEN_HEADER,
                "public-token".to_string()
            ))
        );
        assert_eq!(
            private.graphql_auth_header().await.unwrap(),
            Some((
                STOREFRONT_PRIVATE_ACCESS_TOKEN_HEADER,
                "private-token".to_string()
            ))
        );
        assert_eq!(tokenless.graphql_auth_header().await.unwrap(), None);
    }
}
