pub mod frameworks;
pub mod verify;
pub mod webhook;

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone, Eq, PartialEq)]
pub struct WebhookSubscription {
    pub id: String,
    pub topic: String,
    pub format: String,
    pub uri: String,
}
