use crate::Shopify;
use base64::prelude::*;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

impl Shopify {
    pub fn verify_hmac(&self, data: &[u8], hmac_header: &str) -> bool {
        if let Some(secret) = &self.shared_secret {
            let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
                log::info!("Failed to create webhook HMAC");
                return false;
            };

            let Ok(expected_hmac) = BASE64_STANDARD.decode(hmac_header) else {
                log::info!("Invalid webhook HMAC header encoding");
                return false;
            };

            mac.update(data);
            return mac.verify_slice(&expected_hmac).is_ok();
        }

        log::info!("No shared secret found");

        false
    }
}
