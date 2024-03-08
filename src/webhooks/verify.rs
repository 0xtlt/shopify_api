use crate::Shopify;
use base64;
use base64::prelude::*;
use hmac::{Hmac, Mac};
use sha2::Sha256;

impl Shopify {
    pub fn verify_hmac(&self, data: &[u8], hmac_header: &str) -> bool {
        if let Some(secret) = &self.shared_secret {
            let mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes());

            if mac.is_err() {
                log::info!("Failed to create hmac with {:?}", mac.err().unwrap());
                return false;
            }

            let mut mac = mac.unwrap();

            mac.update(data);
            let result = mac.finalize();
            let calculated_hmac = BASE64_STANDARD.encode(result.into_bytes());

            log::debug!("Calculated HMAC: {}", calculated_hmac);

            return calculated_hmac == hmac_header;
        }

        log::info!("No shared secret found");

        false
    }
}
