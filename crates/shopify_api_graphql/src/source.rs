//! The string tokens of a GraphQL source, found without the parser.
//!
//! `graphql-parser` 0.4 panics on some block strings: it measures the
//! common indentation of their lines in bytes, over any Unicode whitespace,
//! then cuts every line at that byte, which can fall inside a character
//! (a line indented with U+00A0, two bytes, and another with U+2003, three
//! bytes). Those are refused here, before the parser reads them.

use graphql_parser::Pos;

/// The first block string of `source` the parser cannot read, if any, and
/// where it starts.
pub(crate) fn unreadable_block_strings(source: &str) -> Option<Pos> {
    let mut lexer = Lexer::new(source);
    while let Some((position, token)) = lexer.next_string() {
        if unreadable_block_string(token) {
            return Some(position);
        }
    }
    None
}

/// Whether the parser panics on this string token: the same steps as its
/// `unquote_block_string`, without the cut.
pub(crate) fn unreadable_block_string(token: &str) -> bool {
    let Some(body) = token
        .strip_prefix("\"\"\"")
        .and_then(|body| body.strip_suffix("\"\"\""))
    else {
        return false;
    };
    let lines = body.lines();
    let mut common = usize::MAX;
    let mut first = None;
    let mut last = 0;
    for (index, line) in lines.clone().enumerate() {
        let indent = line.len() - line.trim_start().len();
        if indent == line.len() {
            continue;
        }
        first.get_or_insert(index);
        last = index;
        if index != 0 {
            common = common.min(indent);
        }
    }
    let Some(first) = first else {
        return false;
    };
    lines
        .enumerate()
        .skip(first)
        .take(last - first + 1)
        .any(|(index, line)| index != 0 && line.len() >= common && !line.is_char_boundary(common))
}

pub(crate) struct Lexer<'a> {
    source: &'a str,
    /// A byte offset.
    at: usize,
    line: usize,
    column: usize,
}

impl<'a> Lexer<'a> {
    pub(crate) fn new(source: &'a str) -> Self {
        Lexer {
            source,
            at: 0,
            line: 1,
            column: 1,
        }
    }

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

    /// The next string token, quotes included, and where it starts; `None`
    /// at the end, or at a string that does not end.
    pub(crate) fn next_string(&mut self) -> Option<(Pos, &'a str)> {
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

    /// Whether `graphql-parser` panics on the block string `token`.
    fn parser_panics(token: &str) -> bool {
        let source = format!("{{ f(a: {token}) }}");
        std::panic::catch_unwind(|| {
            let _ = graphql_parser::parse_query::<String>(&source);
        })
        .is_err()
    }

    #[test]
    fn unreadable_block_strings_are_those_the_parser_panics_on() {
        let whitespace = [
            " ", "\t", "\u{a0}", "\u{2003}", "\u{3000}", "\u{1680}", "\u{2028}",
        ];
        let mut tokens = Vec::new();
        for a in whitespace {
            for b in whitespace {
                for c in ["", " ", "\u{a0}"] {
                    for line in ["x", ""] {
                        tokens.push(format!("\"\"\"\n{a}{c}x\n{b}{line}\n{c}{a}y\"\"\""));
                        tokens.push(format!("\"\"\"x\n{a}{b}x\n{b}\n{c}{line}\"\"\""));
                    }
                }
            }
        }
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let mismatches: Vec<_> = tokens
            .iter()
            .filter(|token| parser_panics(token) != unreadable_block_string(token))
            .collect();
        std::panic::set_hook(previous);
        assert!(mismatches.is_empty(), "{mismatches:?}");
        assert!(tokens.iter().any(|token| unreadable_block_string(token)));
    }

    #[test]
    fn finds_the_unreadable_block_string() {
        assert_eq!(
            unreadable_block_strings("{ f(a: \"\"\"\n\u{a0}a\n\u{a0}b\"\"\") }"),
            None
        );
        let position = unreadable_block_strings(
            "# \"\"\"\n{ f(a: \"x\", b: \"\"\"\n\u{a0}a\n\u{2003}b\"\"\") }",
        )
        .unwrap();
        assert_eq!((position.line, position.column), (2, 16));
    }
}
