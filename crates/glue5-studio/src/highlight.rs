//! Colours for the snippets the welcome page shows: a shell command, a JSON
//! or a TOML configuration. Small on purpose: what these snippets hold
//! (commands, flags, keys, strings, numbers), not the languages whole.

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum Lang {
    Bash,
    Json,
    Toml,
}

impl Lang {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Lang::Bash => "bash",
            Lang::Json => "json",
            Lang::Toml => "toml",
        }
    }
}

/// `text` in pieces, each with its class (`""`: plain).
pub(crate) fn pieces(lang: Lang, text: &str) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    let mut continued = false;
    for line in text.split_inclusive('\n') {
        // Where a bash command or a TOML key can start (not on a line a
        // backslash continues).
        let mut command = !continued;
        continued = lang == Lang::Bash && line.trim_end().ends_with('\\');
        let mut rest = line;
        while let Some(c) = rest.chars().next() {
            let len = match c {
                '"' | '\'' => quoted(rest, c),
                c if c.is_whitespace() || "{}[],:=|&;\\".contains(c) => c.len_utf8(),
                _ => word(rest),
            };
            let (piece, after) = rest.split_at(len);
            let class = match (lang, c) {
                (_, c) if c.is_whitespace() => "",
                (Lang::Bash | Lang::Toml, '#') => {
                    // A comment: the rest of the line.
                    push(&mut out, "com", rest);
                    break;
                }
                (Lang::Toml, '[') if command => {
                    let end = rest.find(']').map_or(rest.len(), |i| i + 1);
                    push(&mut out, "key", &rest[..end]);
                    rest = &rest[end..];
                    command = false;
                    continue;
                }
                (_, '"' | '\'') if lang == Lang::Json && after.trim_start().starts_with(':') => "key",
                (_, '"' | '\'') => "str",
                (Lang::Bash, '|' | '&' | ';') => {
                    command = true;
                    "punct"
                }
                (Lang::Bash, '-') => "flag",
                (Lang::Bash, _) if command && !piece.contains('=') => {
                    command = false;
                    "cmd"
                }
                (Lang::Toml, _) if command && !"{}[],=".contains(c) => "key",
                (Lang::Json | Lang::Toml, _) if c.is_ascii_digit() || c == '-' => "num",
                (Lang::Json | Lang::Toml, _) if matches!(piece, "true" | "false" | "null") => "kw",
                (Lang::Json | Lang::Toml, _) if "{}[],:=".contains(c) => {
                    command = c == '{' || c == ',';
                    "punct"
                }
                _ => "",
            };
            if lang == Lang::Toml && c == '=' {
                command = false;
            }
            push(&mut out, class, piece);
            rest = after;
        }
    }
    out
}

/// Adds `s` to the last piece when it has the same class.
fn push(out: &mut Vec<(&'static str, String)>, class: &'static str, s: &str) {
    match out.last_mut() {
        Some((c, t)) if *c == class => t.push_str(s),
        _ => out.push((class, s.to_string())),
    }
}

/// The length of the quoted string at the start of `s`, to its closing
/// quote (or the line's end).
fn quoted(s: &str, quote: char) -> usize {
    let mut escaped = false;
    for (i, c) in s.char_indices().skip(1) {
        match c {
            '\\' if !escaped => escaped = true,
            '\n' => return i,
            c if c == quote && !escaped => return i + 1,
            _ => escaped = false,
        }
    }
    s.len()
}

/// The length of the word at the start of `s`: to a space, a quote or a
/// punctuation mark.
fn word(s: &str) -> usize {
    s.find(|c: char| c.is_whitespace() || "\"'{}[],:=|;".contains(c)).filter(|&i| i > 0).unwrap_or(s.len())
}
