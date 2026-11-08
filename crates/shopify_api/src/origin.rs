//! Where the admin of a shop is reached: `https://{shop}`, or another origin
//! that stands in for it, such as a local fake of Shopify.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use crate::{Error, ShopDomain};

/// The scheme, host and port that the addresses built from a shop's domain go
/// to instead of `https://{shop}`: a local stand-in of Shopify, such as
/// [local-storefront](https://github.com/0xtlt/local-storefront), a mock
/// server, or a proxy.
///
/// Only the origin changes. The paths stay Shopify's
/// (`/admin/api/{version}/graphql.json`, `/admin/oauth/access_token`,
/// `/admin/oauth/authorize`), and the shop stays the shop: the session tokens
/// verified, the tokens obtained and the errors are still those of
/// `{shop}.myshopify.com`. The requests sent to the origin say which shop they
/// are for in `X-Forwarded-Host`, and the install link in its `shop`
/// parameter, so that one origin can serve several shops.
///
/// It is an absolute `https://` or `http://` URL: a host and an optional port,
/// no path (a trailing `/` aside), no query string, fragment, user or
/// password. `http://` is taken for a loopback host only, `localhost`,
/// `127.0.0.1` or `[::1]`, so that an access token never travels in clear
/// text off the machine.
///
/// ```
/// use shopify_api::AdminOrigin;
///
/// assert_eq!(AdminOrigin::new("http://localhost:9292/")?.as_str(), "http://localhost:9292");
/// assert!(AdminOrigin::new("https://shopify-proxy.example.com").is_ok());
/// // In clear text, only to this machine.
/// assert!(AdminOrigin::new("http://shopify-proxy.example.com").is_err());
/// // The origin only: the paths are Shopify's.
/// assert!(AdminOrigin::new("http://localhost:9292/admin").is_err());
/// # Ok::<(), shopify_api::Error>(())
/// ```
///
/// Give it to [`ShopifyBuilder::admin_origin`](crate::ShopifyBuilder::admin_origin),
/// or to `ShopifyApp::with_admin_origin` (feature `oauth`) for the token
/// requests of an app and the clients it builds.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AdminOrigin(String);

impl AdminOrigin {
    /// Checks `origin` and writes it as `scheme://host[:port]`, in lower
    /// case, without a trailing `/`. An error does not show `origin`, which
    /// may hold a password.
    pub fn new(origin: &str) -> Result<Self, Error> {
        let (scheme, rest) = origin
            .trim()
            .split_once("://")
            .ok_or_else(|| invalid("not an absolute `http://` or `https://` URL"))?;
        let secure = if scheme.eq_ignore_ascii_case("https") {
            true
        } else if scheme.eq_ignore_ascii_case("http") {
            false
        } else {
            return Err(invalid("the scheme is not `http` or `https`"));
        };
        if rest.contains(['?', '#']) {
            return Err(invalid("it has a query string or a fragment"));
        }
        let authority = rest.strip_suffix('/').unwrap_or(rest);
        if authority.contains('/') {
            return Err(invalid(
                "it has a path: give the scheme, the host and the port only",
            ));
        }
        if authority.contains('@') {
            return Err(invalid("it has a user or a password"));
        }
        let (host, port) = split_port(authority)?;
        let (host, loopback) = host_of(host)?;
        if !secure && !loopback {
            return Err(invalid(
                "`http://` is taken for `localhost`, `127.0.0.1` or `[::1]` only: use `https://`",
            ));
        }
        let scheme = if secure { "https" } else { "http" };
        Ok(Self(match port {
            Some(port) => format!("{scheme}://{host}:{port}"),
            None => format!("{scheme}://{host}"),
        }))
    }

    /// `http://localhost:9292`: no trailing `/`.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The address of `path` (from its `/`) at this origin.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.0)
    }
}

/// The address of `path` (from its `/`) in the admin of `shop`: at `origin`
/// when there is one, else at `https://{shop}`.
pub(crate) fn admin_url(shop: &ShopDomain, origin: Option<&AdminOrigin>, path: &str) -> String {
    match origin {
        Some(origin) => origin.url(path),
        None => format!("{}{path}", shop.origin()),
    }
}

fn invalid(reason: &str) -> Error {
    Error::Config(format!(
        "invalid admin origin, expected `https://host[:port]` (or `http://` to localhost): {reason}"
    ))
}

/// The host and the port of `authority`: `[::1]:9292`, `localhost`.
fn split_port(authority: &str) -> Result<(&str, Option<u16>), Error> {
    let (host, port) = if authority.starts_with('[') {
        let end = authority
            .find(']')
            .ok_or_else(|| invalid("an IPv6 address without its closing `]`"))?;
        let (host, after) = authority.split_at(end + 1);
        match after.strip_prefix(':') {
            Some(port) => (host, Some(port)),
            None if after.is_empty() => (host, None),
            None => return Err(invalid("text after the IPv6 address")),
        }
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    let port = port
        .map(|port| {
            port.bytes()
                .all(|byte| byte.is_ascii_digit())
                .then(|| port.parse::<u16>().ok())
                .flatten()
                .filter(|port| *port != 0)
                .ok_or_else(|| invalid("the port is not a number from 1 to 65535"))
        })
        .transpose()?;
    Ok((host, port))
}

/// The host as it is written in the origin, and whether it is a loopback
/// host `http://` may go to.
fn host_of(host: &str) -> Result<(String, bool), Error> {
    if let Some(inner) = host.strip_prefix('[') {
        let inner = inner.strip_suffix(']').unwrap_or(inner);
        let address = Ipv6Addr::from_str(inner)
            .map_err(|_| invalid("the host is not a valid IPv6 address"))?;
        return Ok((format!("[{address}]"), address == Ipv6Addr::LOCALHOST));
    }
    let host = host.to_ascii_lowercase();
    let label = |label: &str| {
        (1..=63).contains(&label.len())
            && label
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            && !label.starts_with('-')
            && !label.ends_with('-')
    };
    if host.len() > 253 || !host.split('.').all(label) {
        return Err(invalid("the host is not a domain name or an IP address"));
    }
    // A name whose last label is a number is read as an IPv4 address by
    // URL parsers: it must be one, written in the usual way.
    if host
        .rsplit('.')
        .next()
        .is_some_and(|last| last.bytes().all(|byte| byte.is_ascii_digit()))
    {
        let address = Ipv4Addr::from_str(&host)
            .map_err(|_| invalid("the host is not a valid IPv4 address"))?;
        return Ok((address.to_string(), address == Ipv4Addr::LOCALHOST));
    }
    let loopback = host == "localhost";
    Ok((host, loopback))
}

impl fmt::Display for AdminOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl AsRef<str> for AdminOrigin {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl FromStr for AdminOrigin {
    type Err = Error;

    fn from_str(origin: &str) -> Result<Self, Self::Err> {
        Self::new(origin)
    }
}

impl TryFrom<&str> for AdminOrigin {
    type Error = Error;

    fn try_from(origin: &str) -> Result<Self, Self::Error> {
        Self::new(origin)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_origin_without_a_trailing_slash() {
        for (input, expected) in [
            ("http://localhost:9292", "http://localhost:9292"),
            ("http://localhost:9292/", "http://localhost:9292"),
            (" HTTP://LocalHost:9292 ", "http://localhost:9292"),
            ("http://localhost", "http://localhost"),
            ("http://127.0.0.1:9292", "http://127.0.0.1:9292"),
            ("http://[::1]:9292", "http://[::1]:9292"),
            ("http://[0:0:0:0:0:0:0:1]/", "http://[::1]"),
            ("https://lsf.example.com", "https://lsf.example.com"),
            (
                "https://Proxy.Example.com:8443/",
                "https://proxy.example.com:8443",
            ),
            ("https://10.0.0.5:9292", "https://10.0.0.5:9292"),
            ("https://[2001:db8::1]:443", "https://[2001:db8::1]:443"),
            ("https://localhost:09292", "https://localhost:9292"),
        ] {
            let origin = AdminOrigin::new(input).unwrap_or_else(|error| panic!("{input}: {error}"));
            assert_eq!(origin.as_str(), expected, "{input}");
            assert_eq!(input.parse::<AdminOrigin>().unwrap(), origin);
        }
    }

    #[test]
    fn refuses_what_is_not_an_origin() {
        for input in [
            "",
            "localhost:9292",
            "//localhost:9292",
            "ftp://localhost",
            "ws://localhost:9292",
            "https://",
            "http://:9292",
            "http://localhost:9292/admin",
            "http://localhost:9292//",
            "http://localhost:9292/?",
            "http://localhost:9292?shop=x",
            "http://localhost:9292#x",
            "http://user:secret@localhost:9292",
            "http://user@localhost:9292",
            "http://localhost:",
            "http://localhost:0",
            "http://localhost:65536",
            "http://localhost:92a",
            "http://localhost:+92",
            "http://localhost:9292:1",
            "http://::1",
            "http://[::1",
            "http://[::1]x",
            "http://[fe80::1%25en0]",
            "http://[not-an-address]",
            "https://exa mple.com",
            "https://-bad.example.com",
            "https://bad-.example.com",
            "https://a..b",
            "https://example.com.",
            "https://under_score.example.com",
            "https://999.1.1.1",
            "https://127.000.000.001",
            "https://1.2.3",
            "https://exam\\ple.com",
        ] {
            assert!(
                matches!(AdminOrigin::new(input), Err(Error::Config(_))),
                "{input}"
            );
        }
    }

    #[test]
    fn takes_http_for_a_loopback_host_only() {
        for input in [
            "http://example.com",
            "http://lsf.example.com:9292",
            "http://localhost.example.com",
            "http://app.localhost",
            "http://127.0.0.2:9292",
            "http://10.0.0.5:9292",
            "http://0.0.0.0:9292",
            "http://[::ffff:127.0.0.1]:9292",
            "http://[::]:9292",
            "http://2130706433",
        ] {
            let error = AdminOrigin::new(input).unwrap_err();
            assert!(matches!(error, Error::Config(_)), "{input}");
            assert!(
                error.to_string().contains("use `https://`") || error.to_string().contains("IPv4"),
                "{input}: {error}"
            );
            let secure = input.replacen("http://", "https://", 1);
            if input != "http://2130706433" {
                assert!(AdminOrigin::new(&secure).is_ok(), "{secure}");
            }
        }
    }

    #[test]
    fn an_error_does_not_show_the_origin() {
        for input in [
            "http://user:shpat_secret@localhost:9292",
            "http://localhost:9292/?token=shpat_secret",
            "http://shpat_secret.example.com",
            "shpat_secret",
        ] {
            let error = AdminOrigin::new(input).unwrap_err();
            let shown = format!("{error} {error:?}");
            assert!(!shown.contains("shpat_secret"), "{shown}");
        }
    }

    #[test]
    fn builds_the_addresses_of_the_admin() {
        let shop = ShopDomain::new("my-shop").unwrap();
        assert_eq!(
            admin_url(&shop, None, "/admin/oauth/access_token"),
            "https://my-shop.myshopify.com/admin/oauth/access_token"
        );
        let origin = AdminOrigin::new("http://localhost:9292/").unwrap();
        assert_eq!(
            admin_url(&shop, Some(&origin), "/admin/oauth/access_token"),
            "http://localhost:9292/admin/oauth/access_token"
        );
    }
}
