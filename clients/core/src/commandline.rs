//! The commandline's language: what's typed as `key:value`, the same everywhere. Every
//! commandline (a page's, the track editor's, a search, the palette's, the share popup's) is
//! declared once as a `schema::Schema`, which parses it, lists what can be typed where it
//! ends, and says what's missing. The syntax:
//!
//! * `|` separates commands: `status:demo | date:2020`.
//! * A command is a key, its value, and its arguments, in any order: `artist:Ann
//!   role:featured as:Annie`. Another `name:` in a value is split off only when it's one of
//!   the arguments that can follow there, so `title:Intro: Part 2` needs no quotes. A schema
//!   can make every key an argument of every other (a search: `og:yes title:x`, no `|`
//!   needed).
//! * Words before any key are plain words (a search's, a page's actions by name).
//! * `"…"` quotes a value: `title:"x title:y"`, `title:"a | b"`. `\|` is a `|` in a value too.
//! * `-key:value` takes one off; `-key` alone clears it (where the schema says so).
//! * Places: the value is what, `n:` is where (`cover:3 n:1`, `artist:Ann n:2`).
//! * Some commands have fields: after them, any `name:` that isn't one of their arguments is
//!   a field (a source's: `source:distrokid album_uuid:… index:1`).
//!
//! This module splits the text into commands and tokens (`commands_of`); `schema` does the
//! rest.

pub mod schema;

/// A token: where it starts, its text, and whether it starts inside a quoted value.
pub(crate) struct Token<'a> {
    pub(crate) at: usize,
    pub(crate) text: &'a str,
    pub(crate) in_quote: bool,
}

/// The text in tokens, commands apart (`|` outside quotes and not escaped).
pub(crate) fn commands_of<'a>(text: &'a str) -> Vec<Vec<Token<'a>>> {
    let mut commands = vec![Vec::new()];
    let mut start: Option<(usize, bool)> = None;
    let mut in_quote = false;
    let mut chars = text.char_indices().peekable();
    let end_token = |commands: &mut Vec<Vec<Token<'a>>>, start: &mut Option<(usize, bool)>, at: usize| {
        if let Some((s, quoted)) = start.take() {
            commands.last_mut().unwrap().push(Token { at: s, text: &text[s..at], in_quote: quoted });
        }
    };
    while let Some((i, ch)) = chars.next() {
        if ch == '\\' && chars.peek().is_some_and(|(_, c)| *c == '|') {
            // `\|`: part of the token.
            start.get_or_insert((i, in_quote));
            chars.next();
            continue;
        }
        if ch == '"' {
            let token = start.map(|(s, _)| &text[s..i]).unwrap_or("");
            if in_quote {
                in_quote = false;
            } else if opens_quote(token) {
                in_quote = true;
            }
            start.get_or_insert((i, false));
            continue;
        }
        if !in_quote && ch == '|' {
            end_token(&mut commands, &mut start, i);
            commands.push(Vec::new());
            continue;
        }
        if ch.is_whitespace() {
            end_token(&mut commands, &mut start, i);
        } else {
            start.get_or_insert((i, in_quote));
        }
    }
    end_token(&mut commands, &mut start, text.len());
    commands
}

/// Whether a `"` after this much of a token opens a quoted value: right after `key:`.
fn opens_quote(token: &str) -> bool {
    let Some(name) = token.strip_suffix(':') else { return false };
    let name = name.strip_prefix('-').unwrap_or(name);
    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str) -> Vec<Vec<&str>> {
        commands_of(text).into_iter().map(|c| c.into_iter().map(|t| t.text).collect()).collect()
    }

    #[test]
    fn bars_split_commands_unless_quoted_or_escaped() {
        assert_eq!(tokens("a:x | b:y"), [vec!["a:x"], vec!["b:y"]]);
        assert_eq!(tokens("a:x|b:y"), [vec!["a:x"], vec!["b:y"]]);
        assert_eq!(tokens("c:\"a | b\" | d"), [vec!["c:\"a", "|", "b\""], vec!["d"]], "quoted: in the value");
        assert_eq!(tokens("c:a \\| b"), [vec!["c:a", "\\|", "b"]]);
        // A quote inside a value isn't one.
        assert_eq!(tokens("c:12\" vinyl | a:x").len(), 2);
    }
}
