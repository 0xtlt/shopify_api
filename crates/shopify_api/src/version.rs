//! Shopify Admin API versions.

use std::fmt;
use std::str::FromStr;

use crate::Error;

/// A Shopify Admin API version: a quarterly release such as `2026-10`, or `unstable`.
///
/// ```
/// use shopify_api::ApiVersion;
///
/// let version: ApiVersion = "2026-07".parse()?;
/// assert_eq!(version, ApiVersion::V2026_07);
/// assert!(version < ApiVersion::DEFAULT);
/// assert_eq!(ApiVersion::UNSTABLE.to_string(), "unstable");
/// # Ok::<(), shopify_api::Error>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ApiVersion(Repr);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum Repr {
    Dated { year: u16, month: u8 },
    Unstable,
}

impl ApiVersion {
    pub const V2026_04: Self = Self::dated(2026, 4);
    pub const V2026_07: Self = Self::dated(2026, 7);
    pub const V2026_10: Self = Self::dated(2026, 10);
    /// The release candidate of 2027-01.
    pub const V2027_01: Self = Self::dated(2027, 1);
    pub const UNSTABLE: Self = Self(Repr::Unstable);

    /// The version this crate calls when none is chosen: the one of the
    /// schema bundled with it, which `graphql!` checks operations against.
    pub const DEFAULT: Self = Self::V2026_10;

    /// The oldest version this crate accepts: the first whose bulk operation
    /// API (`bulkOperation(id:)`, `bulkOperations`) it relies on.
    pub const MINIMUM: Self = Self::V2026_04;

    const fn dated(year: u16, month: u8) -> Self {
        Self(Repr::Dated { year, month })
    }

    /// Parses `2026-10` or `unstable`. Shopify releases a version each
    /// quarter, in January, April, July and October.
    pub fn new(version: &str) -> Result<Self, Error> {
        let invalid = |reason: &str| Error::InvalidApiVersion {
            version: version.to_string(),
            reason: reason.to_string(),
        };
        if version == "unstable" {
            return Ok(Self::UNSTABLE);
        }
        let bytes = version.as_bytes();
        let well_formed = bytes.len() == 7
            && bytes[4] == b'-'
            && bytes[..4].iter().chain(&bytes[5..]).all(u8::is_ascii_digit);
        if !well_formed {
            return Err(invalid(
                "expected `YYYY-MM`, such as `2026-10`, or `unstable`",
            ));
        }
        let year: u16 = version[..4].parse().map_err(|_| invalid("invalid year"))?;
        let month: u8 = version[5..].parse().map_err(|_| invalid("invalid month"))?;
        if !matches!(month, 1 | 4 | 7 | 10) {
            return Err(invalid(
                "Shopify releases a version in January, April, July and October",
            ));
        }
        let parsed = Self::dated(year, month);
        if parsed < Self::MINIMUM {
            return Err(invalid("the oldest version this crate supports is 2026-04"));
        }
        Ok(parsed)
    }

    pub fn is_unstable(&self) -> bool {
        matches!(self.0, Repr::Unstable)
    }
}

impl Default for ApiVersion {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl fmt::Display for ApiVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Repr::Dated { year, month } => write!(formatter, "{year:04}-{month:02}"),
            Repr::Unstable => formatter.write_str("unstable"),
        }
    }
}

impl FromStr for ApiVersion {
    type Err = Error;

    fn from_str(version: &str) -> Result<Self, Self::Err> {
        Self::new(version)
    }
}

impl TryFrom<&str> for ApiVersion {
    type Error = Error;

    fn try_from(version: &str) -> Result<Self, Self::Error> {
        Self::new(version)
    }
}

impl serde::Serialize for ApiVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for ApiVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::new(&text).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_versions() {
        for text in [
            "2026-04", "2026-07", "2026-10", "2027-01", "2030-10", "unstable",
        ] {
            assert_eq!(ApiVersion::new(text).unwrap().to_string(), text);
        }
        assert_eq!(ApiVersion::DEFAULT.to_string(), "2026-10");
        assert!(ApiVersion::UNSTABLE.is_unstable());
        assert!(ApiVersion::V2026_04 < ApiVersion::V2026_10);
        assert!(ApiVersion::V2027_01 < ApiVersion::UNSTABLE);
    }

    #[test]
    fn refuses_what_shopify_does_not_release() {
        for (text, reason) in [
            (
                "2026-4",
                "expected `YYYY-MM`, such as `2026-10`, or `unstable`",
            ),
            (
                "latest",
                "expected `YYYY-MM`, such as `2026-10`, or `unstable`",
            ),
            (
                "2026-10/../x",
                "expected `YYYY-MM`, such as `2026-10`, or `unstable`",
            ),
            (
                "2026-05",
                "Shopify releases a version in January, April, July and October",
            ),
            (
                "2026-01",
                "the oldest version this crate supports is 2026-04",
            ),
            (
                "2025-10",
                "the oldest version this crate supports is 2026-04",
            ),
        ] {
            match ApiVersion::new(text) {
                Err(Error::InvalidApiVersion {
                    version,
                    reason: given,
                }) => {
                    assert_eq!((version.as_str(), given.as_str()), (text, reason));
                }
                other => panic!("{text}: {other:?}"),
            }
        }
    }

    #[cfg(feature = "cli")]
    #[test]
    fn the_minimum_is_the_one_graphql_checks() {
        assert_eq!(
            ApiVersion::MINIMUM.to_string(),
            shopify_api_graphql::MINIMUM_API_VERSION
        );
        for version in ["2026-04", "2026-07", "2027-10", "unstable"] {
            assert!(
                shopify_api_graphql::is_supported_api_version(version),
                "{version}"
            );
            assert!(ApiVersion::new(version).is_ok(), "{version}");
        }
        for version in ["2026-01", "2025-10", "2026-05", "2026-4", "latest"] {
            assert!(
                !shopify_api_graphql::is_supported_api_version(version),
                "{version}"
            );
            assert!(ApiVersion::new(version).is_err(), "{version}");
        }
    }

    #[test]
    fn travels_as_a_string() {
        let json = serde_json::to_string(&ApiVersion::V2026_07).unwrap();
        assert_eq!(json, "\"2026-07\"");
        assert_eq!(
            serde_json::from_str::<ApiVersion>(&json).unwrap(),
            ApiVersion::V2026_07
        );
        assert!(serde_json::from_str::<ApiVersion>("\"2026-02\"").is_err());
    }
}
