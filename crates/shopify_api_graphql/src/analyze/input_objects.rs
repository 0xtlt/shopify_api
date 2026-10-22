//! Input Object Field Uniqueness: `{ title: "a", title: "b" }` is invalid.
//!
//! The parser keeps input objects in a map, so the second `title` overwrites
//! the first and the analyzer never sees it, while the text Shopify receives
//! still holds both. The check reads the text itself.

use std::collections::HashSet;

use graphql_parser::Pos;

/// The first input field given twice in one input object of `source`, with
/// its position. `source` already parsed: anything unexpected ends the scan.
pub(super) fn duplicate_field(source: &str) -> Option<(&str, Pos)> {
    let mut lexer = Lexer {
        text: source,
        position: 0,
    };
    let mut frames: Vec<Frame<'_>> = Vec::new();
    let mut previous: Option<Token<'_>> = None;
    while let Some((token, start)) = lexer.next() {
        match token {
            Token::Punctuator(b'{') => {
                // A value follows `:` (an argument or a field of an input
                // object) or `=` (a default value), or stands in a list.
                let object = matches!(previous, Some(Token::Punctuator(b':' | b'=')))
                    || matches!(frames.last(), Some(Frame::List));
                frames.push(if object {
                    Frame::Object(HashSet::new())
                } else {
                    Frame::Other
                });
            }
            Token::Punctuator(b'[') => frames.push(Frame::List),
            Token::Punctuator(b'(') => frames.push(Frame::Other),
            Token::Punctuator(b'}' | b']' | b')') => {
                frames.pop()?;
            }
            Token::Name(name) => {
                // In an input object, a name is a field unless it is the value
                // of one (an enum value, `true`, `null`) or of a variable.
                if let Some(Frame::Object(fields)) = frames.last_mut()
                    && !matches!(previous, Some(Token::Punctuator(b':' | b'$')))
                    && !fields.insert(name)
                {
                    return Some((name, position_of(source, start)));
                }
            }
            _ => {}
        }
        previous = Some(token);
    }
    None
}

enum Frame<'a> {
    /// An input object, with the fields given so far.
    Object(HashSet<&'a str>),
    List,
    /// A selection set or arguments.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Name(&'a str),
    Punctuator(u8),
    /// A string, a number, or a punctuator of no use here.
    Other,
}

struct Lexer<'a> {
    text: &'a str,
    position: usize,
}

impl<'a> Lexer<'a> {
    /// The next token and where it starts, `None` at the end.
    fn next(&mut self) -> Option<(Token<'a>, usize)> {
        self.skip_ignored();
        let rest = &self.text.as_bytes()[self.position..];
        let &first = rest.first()?;
        let start = self.position;
        let (token, length) = match first {
            b'A'..=b'Z' | b'a'..=b'z' | b'_' => {
                let length = rest
                    .iter()
                    .position(|byte| !(byte.is_ascii_alphanumeric() || *byte == b'_'))
                    .unwrap_or(rest.len());
                (Token::Name(&self.text[start..start + length]), length)
            }
            b'"' if rest.starts_with(b"\"\"\"") => {
                // A block string: its only escape is `\"""`.
                let mut index = 3;
                loop {
                    let tail = rest.get(index..).filter(|tail| !tail.is_empty())?;
                    if tail.starts_with(b"\\\"\"\"") {
                        index += 4;
                    } else if tail.starts_with(b"\"\"\"") {
                        break;
                    } else {
                        index += 1;
                    }
                }
                (Token::Other, index + 3)
            }
            b'"' => {
                let mut index = 1;
                loop {
                    match rest.get(index)? {
                        b'\\' => index += 2,
                        b'"' => break,
                        _ => index += 1,
                    }
                }
                (Token::Other, index + 1)
            }
            b'{' | b'}' | b'(' | b')' | b'[' | b']' | b':' | b'=' | b'$' => {
                (Token::Punctuator(first), 1)
            }
            b'0'..=b'9' | b'-' => {
                let length = rest
                    .iter()
                    .position(|byte| {
                        !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'+' | b'-'))
                    })
                    .unwrap_or(rest.len());
                (Token::Other, length)
            }
            _ => {
                let length = self.text[start..].chars().next().map_or(1, char::len_utf8);
                (Token::Other, length)
            }
        };
        self.position += length;
        Some((token, start))
    }

    /// Skips white space, line terminators, commas, comments and byte order marks.
    fn skip_ignored(&mut self) {
        loop {
            let rest = &self.text.as_bytes()[self.position..];
            match rest.first() {
                Some(b' ' | b'\t' | b'\n' | b'\r' | b',') => self.position += 1,
                Some(0xEF) if rest.starts_with("\u{feff}".as_bytes()) => self.position += 3,
                Some(b'#') => {
                    self.position += rest
                        .iter()
                        .position(|byte| matches!(byte, b'\n' | b'\r'))
                        .unwrap_or(rest.len());
                }
                _ => return,
            }
        }
    }
}

/// The line and column of the byte `offset` of `source`, both from 1.
fn position_of(source: &str, offset: usize) -> Pos {
    let before = &source[..offset];
    let line_start = before.rfind(['\n', '\r']).map_or(0, |index| index + 1);
    Pos {
        line: before.matches('\n').count() + before.matches('\r').count()
            - before.matches("\r\n").count()
            + 1,
        column: before[line_start..].chars().count() + 1,
    }
}

#[cfg(test)]
mod tests {
    use super::duplicate_field;

    fn duplicate(source: &str) -> Option<(&str, usize, usize)> {
        duplicate_field(source).map(|(name, position)| (name, position.line, position.column))
    }

    #[test]
    fn finds_a_field_given_twice_in_one_input_object() {
        assert_eq!(
            duplicate(r#"mutation { a(x: {id: "1", title: "old", title: "new"}) { id } }"#),
            Some(("title", 1, 41))
        );
        // In a list, nested, in a default value, in a directive, on a later line.
        assert_eq!(
            duplicate("{ a(x: [{b: 1}, {b: 1, b: 2}]) { id } }"),
            Some(("b", 1, 24))
        );
        assert_eq!(duplicate("{ a(x: {b: {c: 1, c: 2}}) }"), Some(("c", 1, 19)));
        assert_eq!(
            duplicate("query($x: In = {a: 1\n a: 2}) { a }"),
            Some(("a", 2, 2))
        );
        assert_eq!(duplicate("{ a @d(x: {k: 1, k: 1}) }"), Some(("k", 1, 18)));
    }

    #[test]
    fn leaves_distinct_fields_and_selections_alone() {
        for source in [
            // The same name in sibling objects, in a nested one, or as a value.
            "{ a(x: [{b: 1}, {b: 2}]) { b b } }",
            "{ a(x: {b: {b: 1}, c: B}) { id } }",
            "{ a(x: {b: B, B: b, c: $b, d: null, null: true}) { id } }",
            // Selection sets repeat fields legally.
            "{ a { id id } b: a { id } }",
            "query($x: [In!] = [{a: 1}]) { a(x: $x) { a a } }",
            // Names in strings, block strings and comments are not fields.
            r#"{ a(x: {b: "b: 1, b: 2", c: """ b: \""" b: """}) { id } }"#,
            "{ a(x: {b: 1 # b: 2\n}) { id } }",
        ] {
            assert_eq!(duplicate(source), None, "{source}");
        }
    }

    #[test]
    fn a_large_input_object_is_scanned_in_linear_time() {
        // Looking each field up among the previous ones took about 50 s for this
        // object in a debug build; a set takes well under one.
        let fields: Vec<String> = (0..100_000)
            .map(|index| format!("f{index}: {index}"))
            .collect();
        let source = format!("{{ a(x: {{{}}}) {{ id }} }}", fields.join(", "));
        let started = std::time::Instant::now();
        assert_eq!(duplicate(&source), None);
        let duplicated = format!("{{ a(x: {{{}, f99999: 0}}) {{ id }} }}", fields.join(", "));
        assert_eq!(
            duplicate(&duplicated).map(|(name, ..)| name),
            Some("f99999")
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }
}
