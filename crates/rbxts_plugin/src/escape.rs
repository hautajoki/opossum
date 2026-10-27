//! Plain text to Luau string-literal text.
//!
//! The renderer takes a [`rbxts_luau::StringLiteral`] value as already-escaped text and only picks the delimiter:
//! `"…"` when the value has no `"` and no newline, else `'…'` when it has no `'`, else a long bracket `[[…]]`. A long
//! bracket does not interpret escapes, so the escaped text must never send the renderer there: newlines are
//! escaped, and when the text has both quote kinds every `"` becomes `\x22`.

use crate::LuauExpr;

/// Escapes plain text so that the renderer, whatever delimiter it picks, produces a Luau literal whose value is
/// exactly `text`:
/// - `\` → `\\`; `\n`, `\r`, `\t` by name; other control characters (and DEL) as `\xHH`, which never runs into a
///   following digit as a decimal `\ddd` would;
/// - `"` → `\x22` when `text` also contains `'` (otherwise the renderer picks the other quote);
/// - everything else, non-ASCII included, as it is.
///
/// Text that needs no escape (identifiers, most ids) comes back unchanged, so the renderer's `obj.key` shortcut for
/// keys that are valid identifiers still applies.
pub fn escape_luau_string(text: &str) -> String {
    let escape_double = text.contains('"') && text.contains('\'');
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '"' if escape_double => out.push_str("\\x22"),
            c if c.is_ascii_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// A Luau string literal with the value `text` (plain text, escaped with [`escape_luau_string`]).
pub fn text_literal(text: &str) -> LuauExpr {
    rbxts_luau::create::string(escape_luau_string(text))
}

/// The Luau string literal roblox-ts emits for a string literal a TS transformer made with
/// `factory.createStringLiteral(text)` (one with no source text), byte for byte. roblox-ts reprints a transformed
/// file and parses it again, so the literal goes through two steps:
/// - TypeScript's printer (`getLiteralText`, `escapeNonAsciiString` with `"`): `\` → `\\`, `"` → `\"`, `\t` `\v` `\f`
///   `\b` `\r` `\n` by name, NUL as `\0` (`\x00` before a digit), other control characters, U+2028, U+2029 and U+0085
///   as `\uXXXX`, and every non-ASCII UTF-16 unit as `\uXXXX` (upper-case hex; a character past U+FFFF as its two
///   surrogates). DEL and the rest of ASCII stay as they are;
/// - roblox-ts's `createStringFromLiteral` on that text, between double quotes: a surrogate pair becomes one
///   `\u{…}` (lower-case hex: `\u{1f600}`), every other `\uXXXX` gets Luau's braces (`\u{00E9}`), the other escapes
///   stay.
///
/// So `"^é.*✓$"` is `"^\u{00E9}.*\u{2713}$"`. [`text_literal`] gives a literal with the same value in a shorter
/// spelling; use this one where the output must match what Flamework's TS transformer gave.
pub fn synthesized_string(text: &str) -> LuauExpr {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // a NUL before a digit would read as an octal escape in strict mode
            '\0' if chars.peek().is_some_and(char::is_ascii_digit) => out.push_str("\\x00"),
            '\0' => out.push_str("\\0"),
            '\t' => out.push_str("\\t"),
            '\u{b}' => out.push_str("\\v"),
            '\u{c}' => out.push_str("\\f"),
            '\u{8}' => out.push_str("\\b"),
            '\r' => out.push_str("\\r"),
            '\n' => out.push_str("\\n"),
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_ascii() && !c.is_ascii_control() || c == '\u{7f}' => out.push(c),
            // TypeScript's two surrogates, which roblox-ts joins into one code point
            c if c as u32 > 0xffff => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push_str(&format!("\\u{{{:04X}}}", c as u32)),
        }
    }
    rbxts_luau::create::string_quoted(out, rbxts_luau::Quote::Double)
}

#[cfg(test)]
mod tests {
    use rbxts_luau::{LateStrings, create, render_expr};

    use super::*;

    fn rendered(text: &str) -> String {
        render_expr(&text_literal(text), &LateStrings::default())
    }

    #[test]
    fn plain_text_is_unchanged() {
        assert_eq!(escape_luau_string("Flamework_abc123"), "Flamework_abc123");
        assert_eq!(escape_luau_string("héllo ☃ 😀"), "héllo ☃ 😀");
        assert_eq!(rendered("plain"), "\"plain\"");
    }

    #[test]
    fn quotes_pick_a_delimiter_or_escape() {
        assert_eq!(rendered("say \"hi\""), "'say \"hi\"'");
        assert_eq!(rendered("it's"), "\"it's\"");
        assert_eq!(rendered("it's \"x\""), "\"it's \\x22x\\x22\"");
    }

    #[test]
    fn backslashes_newlines_and_controls() {
        assert_eq!(rendered("a\\b"), "\"a\\\\b\"");
        assert_eq!(rendered("a\nb\r\tc"), "\"a\\nb\\r\\tc\"");
        // \x00 then a digit: a decimal escape \0 would have read "\01"
        assert_eq!(rendered("\u{0}1\u{7f}"), "\"\\x001\\x7f\"");
        // never a long bracket, which would not interpret escapes
        assert!(!rendered("both ' and \" and \n and \\").contains("[["));
    }

    #[test]
    fn synthesized_strings_are_spelled_as_roblox_ts_spells_a_factory_literal() {
        let rendered = |text: &str| render_expr(&synthesized_string(text), &LateStrings::default());
        // (each expectation is stock roblox-ts's output for `factory.createStringLiteral(text)`)
        assert_eq!(rendered("^é.*✓$"), r#""^\u{00E9}.*\u{2713}$""#);
        assert_eq!(rendered("plain_id"), r#""plain_id""#);
        assert_eq!(rendered("it's \"q\" \\ /"), r#""it's \"q\" \\ /""#);
        assert_eq!(rendered("a\nb\r\tc\u{b}\u{c}\u{8}"), r#""a\nb\r\tc\v\f\b""#);
        assert_eq!(rendered("\u{0}x\u{0}7\u{1b}\u{7f}"), "\"\\0x\\x007\\u{001B}\u{7f}\"");
        assert_eq!(rendered("😀\u{2028}\u{2029}\u{85}"), r#""\u{1f600}\u{2028}\u{2029}\u{0085}""#);
        assert_eq!(rendered("tab\there\u{1}\u{1f}"), r#""tab\there\u{0001}\u{001F}""#);
        assert_eq!(rendered("ß€𝄞\u{a0}\u{feff}"), r#""\u{00DF}\u{20AC}\u{1d11e}\u{00A0}\u{FEFF}""#);
        assert_eq!(rendered("${x}`"), r#""${x}`""#);
    }

    #[test]
    fn late_strings_render_escaped_values_as_keys_and_values() {
        let mut late = LateStrings::new();
        late.insert("id", escape_luau_string("ok_id"));
        late.insert("odd", escape_luau_string("x\"'y"));
        let tree = vec![create::local(
            create::any_id("t"),
            Some(create::map([
                (create::late_string("id"), create::late_string("odd")),
                (create::late_string("odd"), create::number(1.0)),
            ])),
        )];
        let out = rbxts_luau::render(&tree, &late);
        assert!(out.contains("ok_id = \"x\\x22'y\""), "{out}");
        assert!(out.contains("[\"x\\x22'y\"] = 1"), "{out}");
    }
}
