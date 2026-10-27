//! The domain of a shop, which every request and every token is bound to.

use std::fmt;
use std::str::FromStr;

use crate::Error;

const SUFFIX: &str = ".myshopify.com";

/// The permanent domain of a shop: `my-shop.myshopify.com`.
///
/// Requests and tokens go to the address built from it, so it only takes a
/// shop's handle or its `myshopify.com` domain, never another host:
///
/// ```
/// use shopify_api::ShopDomain;
///
/// assert_eq!(ShopDomain::new("my-shop")?.as_str(), "my-shop.myshopify.com");
/// assert_eq!(ShopDomain::new("https://My-Shop.myshopify.com/")?.handle(), "my-shop");
/// assert!(ShopDomain::new("my-shop.example.com").is_err());
/// assert!(ShopDomain::new("evil.com/x?.myshopify.com").is_err());
/// # Ok::<(), shopify_api::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShopDomain(String);

impl ShopDomain {
    /// Accepts `my-shop`, `my-shop.myshopify.com`, with or without `https://`
    /// and a trailing `/`, in any case.
    pub fn new(shop: &str) -> Result<Self, Error> {
        let invalid = || Error::InvalidShop(shop.to_string());
        let trimmed = shop.trim();
        let trimmed = trimmed.strip_prefix("https://").unwrap_or(trimmed);
        let trimmed = trimmed.strip_suffix('/').unwrap_or(trimmed);
        let lowered = trimmed.to_ascii_lowercase();
        let handle = lowered.strip_suffix(SUFFIX).unwrap_or(&lowered);
        let valid = !handle.is_empty()
            && handle.len() <= 63
            && handle
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !handle.starts_with('-')
            && !handle.ends_with('-');
        if !valid {
            return Err(invalid());
        }
        Ok(Self(format!("{handle}{SUFFIX}")))
    }

    /// `my-shop.myshopify.com`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `my-shop`.
    pub fn handle(&self) -> &str {
        &self.0[..self.0.len() - SUFFIX.len()]
    }

    /// `https://my-shop.myshopify.com`.
    pub fn origin(&self) -> String {
        format!("https://{}", self.0)
    }
}

impl fmt::Display for ShopDomain {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for ShopDomain {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for ShopDomain {
    type Err = Error;

    fn from_str(shop: &str) -> Result<Self, Self::Err> {
        Self::new(shop)
    }
}

impl TryFrom<&str> for ShopDomain {
    type Error = Error;

    fn try_from(shop: &str) -> Result<Self, Self::Error> {
        Self::new(shop)
    }
}

impl serde::Serialize for ShopDomain {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> serde::Deserialize<'de> for ShopDomain {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::new(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_handles_and_domains() {
        for input in [
            "my-shop",
            "my-shop.myshopify.com",
            "MY-SHOP.myshopify.com",
            " https://my-shop.myshopify.com/ ",
            "my-shop.MyShopify.com",
        ] {
            let shop = ShopDomain::new(input).unwrap();
            assert_eq!(shop.as_str(), "my-shop.myshopify.com", "{input}");
            assert_eq!(shop.handle(), "my-shop");
            assert_eq!(shop.origin(), "https://my-shop.myshopify.com");
        }
        assert_eq!(
            ShopDomain::new("shop2").unwrap().as_str(),
            "shop2.myshopify.com"
        );
    }

    #[test]
    fn refuses_any_other_host() {
        for input in [
            "",
            ".myshopify.com",
            "my shop",
            "my_shop",
            "-shop",
            "shop-",
            "my-shop.example.com",
            "http://my-shop.myshopify.com",
            "my-shop.myshopify.com.evil.com",
            "evil.com/x?.myshopify.com",
            "evil.com#.myshopify.com",
            "user@my-shop.myshopify.com",
            "my-shop.myshopify.com:8443",
            "a.b.myshopify.com",
        ] {
            assert!(
                matches!(ShopDomain::new(input), Err(Error::InvalidShop(_))),
                "{input}"
            );
        }
    }
}
