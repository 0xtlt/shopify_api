//! The strings of a bulk query read as GraphQL reads them.
//!
//! A bulk query is sent as text written back from what `graphql-parser`
//! read, which decodes some strings unlike GraphQL: `\b` as U+0010, not
//! U+0008, and block strings split and indented with Unicode whitespace and
//! without a lone `\r` as a line break. Its values carry no position, and
//! the fields of an input object are kept sorted, so the text of a string
//! cannot be found back for its value. Each string of the source is
//! decoded here as the GraphQL specification does, and a string the
//! parser reads otherwise is refused.

use super::error_at;
use crate::analyze::Error;
use graphql_parser::Pos;

/// Refuses a string or block string of `source` that `graphql-parser`
/// does not read as GraphQL does.
pub(super) fn check(source: &str) -> Result<(), Error> {
    let mut lexer = Lexer {
        source,
        at: 0,
        line: 1,
        column: 1,
    };
    while let Some((position, token)) = lexer.next_string() {
        let expected = if token.starts_with("\"\"\"") {
            block_string(&token[3..token.len() - 3])
        } else {
            quoted_string(&token[1..token.len() - 1])
        };
        if expected.is_some() && expected == parsed(token) {
            continue;
        }
        let advice = if token.starts_with("\"\"\"") {
            "Write it as a quoted string, with escapes"
        } else if token.contains("\\b") {
            "Write `\\u0008` rather than `\\b`"
        } else {
            "Write the characters it holds with `\\uXXXX` escapes"
        };
        return error_at(
            position,
            format!(
                "This string would not reach Shopify as written: the GraphQL parser this crate uses reads it otherwise, and a bulk query is sent as the text it read. {advice}"
            ),
        );
    }
    Ok(())
}

/// The string the token is for `graphql-parser`.
fn parsed(token: &str) -> Option<String> {
    use graphql_parser::query::{Definition, OperationDefinition, Selection, Value};
    let source = format!("{{ f(a: {token}) }}");
    let document = graphql_parser::parse_query::<String>(&source).ok()?;
    let Some(Definition::Operation(OperationDefinition::SelectionSet(set))) =
        document.definitions.into_iter().next()
    else {
        return None;
    };
    let Some(Selection::Field(field)) = set.items.into_iter().next() else {
        return None;
    };
    match field.arguments.into_iter().next() {
        Some((_, Value::String(value))) => Some(value),
        _ => None,
    }
}

struct Lexer<'a> {
    source: &'a str,
    /// A byte offset.
    at: usize,
    line: usize,
    column: usize,
}

impl<'a> Lexer<'a> {
    fn rest(&self) -> &'a str {
        &self.source[self.at..]
    }

    /// Moves `bytes` on, counting lines as the parser does.
    fn advance(&mut self, bytes: usize) {
        let end = self.at + bytes;
        for character in self.source[self.at..end].chars() {
            if character == '\n' {
                self.line += 1;
                self.column = 1;
            } else if character == '\t' {
                self.column += 8;
            } else {
                self.column += 1;
            }
        }
        self.at = end;
    }

    /// The next string token, quotes included, and where it starts. The
    /// source parsed: every string ends.
    fn next_string(&mut self) -> Option<(Pos, &'a str)> {
        loop {
            let rest = self.rest();
            let character = rest.chars().next()?;
            match character {
                '#' => {
                    let end = rest.find(['\n', '\r']).unwrap_or(rest.len());
                    self.advance(end);
                }
                '"' => {
                    let position = Pos {
                        line: self.line,
                        column: self.column,
                    };
                    let length = string_length(rest)?;
                    let token = &rest[..length];
                    self.advance(length);
                    return Some((position, token));
                }
                other => self.advance(other.len_utf8()),
            }
        }
    }
}

/// The length in bytes of the string token at the start of `rest`.
fn string_length(rest: &str) -> Option<usize> {
    if let Some(body) = rest.strip_prefix("\"\"\"") {
        let mut from = 0;
        loop {
            let end = from + body[from..].find("\"\"\"")?;
            if body[..end].ends_with('\\') {
                from = end + 3;
            } else {
                return Some(end + 6);
            }
        }
    }
    let mut escaped = false;
    for (index, character) in rest.char_indices().skip(1) {
        match character {
            '"' if !escaped => return Some(index + 1),
            _ => escaped = !escaped && character == '\\',
        }
    }
    None
}

/// The value of a quoted string, as the specification reads it.
fn quoted_string(body: &str) -> Option<String> {
    let mut value = String::with_capacity(body.len());
    let mut characters = body.chars();
    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                let escaped = match characters.next()? {
                    '"' => '"',
                    '\\' => '\\',
                    '/' => '/',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    'u' => {
                        let hex: String = characters.by_ref().take(4).collect();
                        if hex.len() != 4 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                            return None;
                        }
                        char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?
                    }
                    _ => return None,
                };
                value.push(escaped);
            }
            // A line terminator ends a line, not a string.
            '\n' | '\r' => return None,
            other => value.push(other),
        }
    }
    Some(value)
}

/// The value of a block string, as the specification reads it.
fn block_string(body: &str) -> Option<String> {
    let raw = body.replace("\\\"\"\"", "\"\"\"");
    let raw = raw.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = raw.split('\n').collect();
    let indent = |line: &str| line.len() - line.trim_start_matches([' ', '\t']).len();
    let common = lines
        .iter()
        .skip(1)
        .filter(|line| indent(line) < line.len())
        .map(|line| indent(line))
        .min();
    let lines: Vec<&str> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| match common {
            Some(common) if index > 0 => line.get(common..).unwrap_or(""),
            _ => line,
        })
        .collect();
    let blank = |line: &&str| line.trim_start_matches([' ', '\t']).is_empty();
    let first = lines.iter().position(|line| !blank(line));
    let last = lines.iter().rposition(|line| !blank(line));
    match (first, last) {
        (Some(first), Some(last)) => Some(lines[first..=last].join("\n")),
        _ => Some(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_end_where_the_parser_ends_them() {
        assert_eq!(string_length(r#""a\"b" x"#), Some(6));
        assert_eq!(string_length(r#""a\\" x"#), Some(5));
        assert_eq!(string_length(r#""""a\"""b""" x"#), Some(12));
        assert_eq!(string_length(r#""""""" x"#), Some(6));
    }

    #[test]
    fn comments_hide_quotes() {
        assert!(check("# \"\\b\"\n{ f }").is_ok());
        assert!(check("{ f(a: \"# \\b\") }").is_err());
    }

    #[test]
    fn the_parser_is_checked_for_every_escape() {
        for escape in [
            r"\u0008", r#"\""#, r"\\", r"\/", r"\f", r"\n", r"\r", r"\t", r"\u00e9", r"\uFFFF",
        ] {
            let source = format!("{{ f(a: \"x{escape}y\") }}");
            assert!(check(&source).is_ok(), "{escape}");
        }
        let error = check("{ f(a: \"x\\by\") }").unwrap_err();
        assert!(error.message.contains("`\\u0008`"), "{}", error.message);
        assert_eq!(error.position, Some((1, 8)));
    }

    #[test]
    fn block_strings_follow_the_specification() {
        assert_eq!(
            block_string("\n    a\n      b\n    \\\"\"\"\n  "),
            Some("a\n  b\n\"\"\"".to_string())
        );
        assert_eq!(block_string("a\rb"), Some("a\nb".to_string()));
        assert!(check("{ f(a: \"\"\"\n    a\n      b\n  \"\"\") }").is_ok());
        assert!(check("{ f(a: \"\"\"a\rb\"\"\") }").is_err());
        assert!(check("{ f(a: \"\"\"\n\u{a0}a\n\u{a0}b\"\"\") }").is_err());
    }
}
