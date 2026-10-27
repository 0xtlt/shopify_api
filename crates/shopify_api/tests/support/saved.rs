//! A `TokenStore` that keeps what it is given, for the tests of tokens.

use std::sync::{Arc, Mutex};

use shopify_api::transport::BoxFuture;
use shopify_api::{ShopDomain, TokenData, TokenStore};

#[derive(Default, Clone)]
pub struct Saved(pub Arc<Mutex<Vec<(String, TokenData)>>>);

impl TokenStore for Saved {
    fn save<'a>(
        &'a self,
        shop: &'a ShopDomain,
        token: &'a TokenData,
    ) -> BoxFuture<'a, Result<(), Box<dyn std::error::Error + Send + Sync>>> {
        Box::pin(async move {
            self.0
                .lock()
                .unwrap()
                .push((shop.to_string(), token.clone()));
            Ok(())
        })
    }
}
