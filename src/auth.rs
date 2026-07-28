use std::{future::Future, pin::Pin};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{Shopify, ShopifyAPIError};

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
}
