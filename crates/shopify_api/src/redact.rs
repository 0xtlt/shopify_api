//! What an error keeps of a text that may hold a secret: a serde message
//! without the values it quotes, a body without addresses' query strings,
//! tokens or signatures.

/// The longest body an [`Error::Http`](crate::Error::Http) shows, in bytes.
pub(crate) const BODY_LIMIT: usize = 1024;

/// What is read of a body before it is redacted, then cut at [`BODY_LIMIT`].
const BODY_SCAN: usize = 16 * 1024;

const REDACTED: &str = "[redacted]";

/// A serde or serde_json message without the values it quotes:
/// `invalid type: string "…", expected u64` becomes
/// `invalid type: string, expected u64`. What the type expects, the names of
/// its fields and the position in the text stay.
///
/// A message of another shape, which a type's own `Deserialize` wrote, loses
/// whatever it quotes between `"` or backticks.
pub(crate) fn serde_message(message: &str) -> String {
    for prefix in ["invalid type: ", "invalid value: "] {
        if let Some(rest) = message.strip_prefix(prefix) {
            let (kind, rest) = unexpected(rest);
            return format!("{prefix}{kind}{rest}");
        }
    }
    for (prefix, separators) in [
        (
            "unknown variant `",
            ["`, expected ", "`, there are no variants"],
        ),
        (
            "unknown field `",
            ["`, expected ", "`, there are no fields"],
        ),
    ] {
        if let Some(rest) = message.strip_prefix(prefix) {
            // The name is the input's, and may hold the separator: the last
            // one is serde's.
            if let Some(end) = separators
                .iter()
                .filter_map(|separator| rest.rfind(separator))
                .max()
            {
                let what = prefix.trim_end_matches(" `");
                return format!("{what}{}", &rest[end + 1..]);
            }
            return prefix.trim_end_matches(" `").to_string();
        }
    }
    // `missing field `name``, `duplicate field `name``, `invalid length 3,
    // expected …`: names of the type, counts. serde_json's syntax errors
    // quote one punctuator at most.
    if message.starts_with("missing field `")
        || message.starts_with("duplicate field `")
        || message.starts_with("invalid length ")
    {
        return message.to_string();
    }
    without_quoted(message)
}

/// The kind of an `Unexpected` value without the value, and what follows it.
fn unexpected(text: &str) -> (&'static str, &str) {
    // `string "…"`: the string as `Debug` writes it, quotes escaped.
    if let Some(rest) = text.strip_prefix("string \"") {
        let mut escaped = false;
        for (index, character) in rest.char_indices() {
            match character {
                '\\' if !escaped => escaped = true,
                '"' if !escaped => return ("string", &rest[index + 1..]),
                _ => escaped = false,
            }
        }
        return ("string", "");
    }
    for kind in ["boolean", "integer", "floating point", "character"] {
        if let Some(rest) = text
            .strip_prefix(kind)
            .and_then(|rest| rest.strip_prefix(" `"))
        {
            // A character may be a backtick itself: skip one before looking.
            let skip = if kind == "character" {
                rest.chars().next().map_or(0, char::len_utf8)
            } else {
                0
            };
            return match rest[skip..].find('`') {
                Some(end) => (kind, &rest[skip + end + 1..]),
                None => (kind, ""),
            };
        }
    }
    for kind in [
        "byte array",
        "unit value",
        "Option value",
        "newtype struct",
        "sequence",
        "map",
        "enum",
        "unit variant",
        "newtype variant",
        "tuple variant",
        "struct variant",
        "null",
    ] {
        if let Some(rest) = text.strip_prefix(kind)
            && (rest.is_empty() || rest.starts_with(','))
        {
            return (kind, rest);
        }
    }
    // `Unexpected::Other`, written by the type: its text up to serde's
    // separator is left out.
    match text.rfind(", expected ") {
        Some(end) => ("a value", &text[end..]),
        None => ("a value", ""),
    }
}

/// `text` without what it quotes between `"` or backticks, except a single
/// character (serde_json's ``expected `,` or `}` ``).
fn without_quoted(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(['"', '`']) {
        let quote = rest[start..].chars().next().unwrap_or('"');
        kept.push_str(&rest[..=start]);
        let inside = &rest[start + 1..];
        let Some(end) = inside.find(quote) else {
            kept.push('…');
            kept.push(quote);
            return kept;
        };
        if inside[..end].chars().count() <= 1 {
            kept.push_str(&inside[..end]);
        } else {
            kept.push('…');
        }
        kept.push(quote);
        rest = &inside[end + 1..];
    }
    kept.push_str(rest);
    kept
}

/// The body of an answer as an error shows it: at most [`BODY_LIMIT`] bytes,
/// with the query string of each address, anything that looks like a token,
/// and the value of each entry whose name holds `token`, `secret`,
/// `signature`, `password`, `credential`, `authorization` or `api_key`
/// replaced by `[redacted]`.
pub(crate) fn http_body(body: &[u8]) -> String {
    let scanned = &body[..body.len().min(BODY_SCAN)];
    let mut text = String::from_utf8_lossy(scanned).into_owned();
    if scanned.len() < body.len() {
        // Do not show the start of a word cut in two: it may be a token
        // whose end would no longer look like one.
        let end = text
            .rfind(|character: char| !is_word(character))
            .unwrap_or(0);
        text.truncate(end);
    }
    let text = redact(&text);
    if text.len() <= BODY_LIMIT && scanned.len() == body.len() {
        return text;
    }
    let mut end = text.len().min(BODY_LIMIT);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

/// Characters of a token, a key or an encoded value.
fn is_word(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || matches!(character, '_' | '-' | '.' | '~' | '+' | '/' | '=' | '%')
}

/// Names of entries whose value is a secret, lower case.
const SECRET_NAMES: [&str; 9] = [
    "token",
    "secret",
    "signature",
    "password",
    "credential",
    "authorization",
    "apikey",
    "api_key",
    "api-key",
];

fn redact(text: &str) -> String {
    let text = redact_query_strings(text);
    let text = redact_entries(&text);
    redact_tokens(&text)
}

/// Each `http://` or `https://` address keeps its path, not its query string
/// or fragment.
fn redact_query_strings(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = find_ignoring_case(rest, "http://")
        .into_iter()
        .chain(find_ignoring_case(rest, "https://"))
        .min()
    {
        let end = rest[start..]
            .find(|character: char| {
                character.is_whitespace()
                    || matches!(character, '"' | '\'' | '<' | '>' | '\\' | '`')
            })
            .map_or(rest.len(), |end| start + end);
        let address = &rest[start..end];
        kept.push_str(&rest[..start]);
        match address.find(['?', '#']) {
            Some(query) => {
                kept.push_str(&address[..=query]);
                kept.push_str(REDACTED);
            }
            None => kept.push_str(address),
        }
        rest = &rest[end..];
    }
    kept.push_str(rest);
    kept
}

/// The value after a secret's name and `:` or `=`, quoted or not
/// (`"access_token": "…"`, `X-Goog-Signature=…`, `Authorization: Bearer …`).
fn redact_entries(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut kept = String::with_capacity(text.len());
    let mut copied = 0;
    let mut search = 0;
    while let Some((name_end, value_start, value_end)) = next_entry(text, &lower, search) {
        if value_start < copied {
            search = name_end;
            continue;
        }
        kept.push_str(&text[copied..value_start]);
        kept.push_str(REDACTED);
        copied = value_end;
        search = value_end.max(name_end);
    }
    kept.push_str(&text[copied..]);
    kept
}

/// The next secret entry from `from`: where its name ends, and where its
/// value starts and ends.
fn next_entry(text: &str, lower: &str, from: usize) -> Option<(usize, usize, usize)> {
    let bytes = text.as_bytes();
    let mut position = from;
    loop {
        let (start, name) = SECRET_NAMES
            .iter()
            .filter_map(|name| Some((lower[position..].find(name)? + position, *name)))
            .min()?;
        let mut cursor = start + name.len();
        // The rest of the name: `access_token`, `X-Goog-Signature`, `tokens`.
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || matches!(bytes[cursor], b'_' | b'-'))
        {
            cursor += 1;
        }
        let name_end = cursor;
        // A closing quote, spaces, then `:` or `=`.
        if cursor < bytes.len() && matches!(bytes[cursor], b'"' | b'\'') {
            cursor += 1;
        }
        while cursor < bytes.len() && bytes[cursor] == b' ' {
            cursor += 1;
        }
        if cursor >= bytes.len() || !matches!(bytes[cursor], b':' | b'=') {
            position = name_end;
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor] == b' ' {
            cursor += 1;
        }
        // `Bearer abc`: the scheme stays, the credential goes.
        for scheme in ["bearer ", "basic "] {
            if lower[cursor..].starts_with(scheme) {
                cursor += scheme.len();
                while cursor < bytes.len() && bytes[cursor] == b' ' {
                    cursor += 1;
                }
            }
        }
        let value_start = cursor;
        let value_end = match bytes.get(cursor) {
            Some(&quote @ (b'"' | b'\'')) => {
                let mut end = cursor + 1;
                while end < bytes.len() && bytes[end] != quote {
                    end += if bytes[end] == b'\\' { 2 } else { 1 };
                }
                // The quotes stay; what is between them goes.
                return Some((name_end, value_start + 1, end.min(bytes.len())));
            }
            _ => {
                let mut end = cursor;
                while end < bytes.len()
                    && !bytes[end].is_ascii_whitespace()
                    && !matches!(
                        bytes[end],
                        b'&' | b',' | b';' | b'"' | b'\'' | b'<' | b'}' | b']' | b')'
                    )
                {
                    end += 1;
                }
                end
            }
        };
        if value_end == value_start {
            position = name_end;
            continue;
        }
        return Some((name_end, value_start, value_end));
    }
}

/// Words that look like a credential: Shopify's tokens and secrets
/// (`shpat_…`, `shpss_…`), a JWT, or 32 characters or more of letters and
/// digits mixed.
fn redact_tokens(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut rest = text;
    while !rest.is_empty() {
        let start = rest.find(is_word).unwrap_or(rest.len());
        kept.push_str(&rest[..start]);
        rest = &rest[start..];
        let end = rest
            .find(|character| !is_word(character))
            .unwrap_or(rest.len());
        let word = &rest[..end];
        if looks_like_a_credential(word) {
            kept.push_str(REDACTED);
        } else {
            kept.push_str(word);
        }
        rest = &rest[end..];
    }
    kept
}

fn looks_like_a_credential(word: &str) -> bool {
    let bytes = word.as_bytes();
    let shopify = bytes.len() > 6
        && bytes.starts_with(b"shp")
        && bytes[3..5].iter().all(u8::is_ascii_lowercase)
        && bytes[5] == b'_';
    let jwt = word.starts_with("eyJ") && word.matches('.').count() >= 2;
    let mixed = word.len() >= 32
        && word.bytes().any(|byte| byte.is_ascii_alphabetic())
        && word.bytes().any(|byte| byte.is_ascii_digit())
        && word != REDACTED;
    shopify || jwt || mixed
}

fn find_ignoring_case(text: &str, needle: &str) -> Option<usize> {
    text.as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_messages_keep_kinds_and_lose_values() {
        for (message, expected) in [
            (
                r#"invalid type: string "https://x.test/r?sig=SECRET", expected u64"#,
                "invalid type: string, expected u64",
            ),
            (
                r#"invalid type: string "a \"quoted\", expected x", expected u64 at line 1 column 9"#,
                "invalid type: string, expected u64 at line 1 column 9",
            ),
            (
                "invalid type: integer `4242`, expected a string",
                "invalid type: integer, expected a string",
            ),
            (
                "invalid type: floating point `1.5`, expected u32",
                "invalid type: floating point, expected u32",
            ),
            (
                "invalid type: boolean `true`, expected a string",
                "invalid type: boolean, expected a string",
            ),
            (
                "invalid type: character ```, expected a string",
                "invalid type: character, expected a string",
            ),
            (
                "invalid type: map, expected a sequence",
                "invalid type: map, expected a sequence",
            ),
            (
                "invalid type: null, expected a string",
                "invalid type: null, expected a string",
            ),
            (
                r#"invalid value: string "SECRET", expected a date"#,
                "invalid value: string, expected a date",
            ),
            (
                "invalid value: SECRET thing, expected a date",
                "invalid value: a value, expected a date",
            ),
            (
                "unknown variant `SECRET`, expected one of `ACTIVE`, `DRAFT`",
                "unknown variant, expected one of `ACTIVE`, `DRAFT`",
            ),
            (
                "unknown variant `SE`, expected `CRET`, expected `A` or `B`",
                "unknown variant, expected `A` or `B`",
            ),
            (
                "unknown field `SECRET`, there are no fields",
                "unknown field, there are no fields",
            ),
            ("missing field `name`", "missing field `name`"),
            (
                "invalid length 3, expected a tuple of size 2",
                "invalid length 3, expected a tuple of size 2",
            ),
            (
                "expected `,` or `}` at line 1 column 9",
                "expected `,` or `}` at line 1 column 9",
            ),
            (
                "invalid shop `SECRET`: expected `<name>`",
                "invalid shop `…`: expected `…`",
            ),
            (r#"bad "SECRET"#, r#"bad "…""#),
        ] {
            assert_eq!(serde_message(message), expected, "{message}");
        }
    }

    #[test]
    fn bodies_lose_query_strings_tokens_and_signatures() {
        let body = concat!(
            r#"{"errors":"Invalid token","url":"https://storage.googleapis.com/b/r.jsonl?X-Goog-Signature=AAA111&X-Goog-Credential=BBB","#,
            r#""access_token": "shpat_0123456789abcdef", "refresh_token":'shprt_x1', "#,
            "plain shpss_00112233 jwt eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.c2lnbmF0dXJl ",
            "random 4f9a8c7e6d5b4a39281706f5e4d3c2b1a0 ",
            "Authorization: Bearer abc.DEF-123 next ",
            "<StringToSign>GOOG4-RSA-SHA256</StringToSign> X-Goog-Signature=CCC&b=1 ",
            "id gid://shopify/Product/1 request a1b2c3d4-e5f6",
        );
        let shown = http_body(body.as_bytes());
        for secret in [
            "AAA111",
            "BBB",
            "shpat_",
            "shprt_",
            "shpss_",
            "eyJ",
            "4f9a8c7e6d5b4a39281706f5e4d3c2b1a0",
            "abc.DEF-123",
            "CCC",
        ] {
            assert!(!shown.contains(secret), "{secret} in {shown}");
        }
        for kept in [
            "Invalid token",
            "https://storage.googleapis.com/b/r.jsonl?[redacted]",
            r#""access_token": "[redacted]""#,
            "Bearer [redacted] next",
            "gid://shopify/Product/1",
            "a1b2c3d4-e5f6",
            "next",
        ] {
            assert!(shown.contains(kept), "{kept} not in {shown}");
        }
    }

    #[test]
    fn bodies_are_cut_at_one_kibibyte() {
        let shown = http_body("é".repeat(2000).as_bytes());
        assert!(shown.ends_with('…'));
        assert!(shown.len() <= BODY_LIMIT + '…'.len_utf8());
        // Tokens shrink to `[redacted]`: one cut at the end of what is read
        // would show its start, which no longer looks like a token.
        let token = format!("{} ", "a1".repeat(150));
        let body = token.repeat(BODY_SCAN / token.len() + 2);
        let shown = http_body(body.as_bytes());
        assert!(shown.starts_with("[redacted] [redacted]"), "{shown}");
        assert!(!shown.contains("a1"), "{shown}");
        assert_eq!(http_body(b"short"), "short");
    }
}
