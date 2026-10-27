//! `util/createStringFromLiteral.ts`: the source spelling of a string or template literal, except for escapes with a
//! different syntax or meaning in Luau.

use tsrs_ast::{Kind, Node};
use tsrs_core::P;

use crate::util::traversal::get_text;

fn is_hex(c: u8) -> bool {
    c.is_ascii_hexdigit()
}

/// `\uD8xx\uDCxx` at `i` (after the backslash at `i - 1`): the two hex quads, when they are a surrogate pair.
fn surrogate_pair_at(bytes: &[u8], i: usize) -> Option<(u16, u16)> {
    // u H H H H \ u H H H H
    let s = bytes.get(i..i + 11)?;
    if s[0] != b'u' || s[5] != b'\\' || s[6] != b'u' {
        return None;
    }
    let high = &s[1..5];
    let low = &s[7..11];
    if !high.iter().chain(low).all(|&c| is_hex(c)) {
        return None;
    }
    let ok_high = matches!(high[0], b'd' | b'D') && matches!(high[1], b'8' | b'9' | b'a' | b'A' | b'b' | b'B');
    let ok_low = matches!(low[0], b'd' | b'D') && matches!(low[1], b'c'..=b'f' | b'C'..=b'F');
    if !(ok_high && ok_low) {
        return None;
    }
    let parse = |h: &[u8]| u16::from_str_radix(std::str::from_utf8(h).unwrap(), 16).unwrap();
    Some((parse(high), parse(low)))
}

/// `createStringFromLiteral`
pub fn create_string_from_literal(node: P<Node>) -> String {
    // node.text decodes escapes, which could turn \b into \b and give it a different meaning in Luau
    let source = get_text(node);

    // every token starts with a quote, backtick, or }, and ends with one delimiter except heads and middles
    // ending in ${
    let end_trim = if matches!(node.kind(), Kind::TemplateHead | Kind::TemplateMiddle) { 2 } else { 1 };
    let text = &source[1.min(source.len())..source.len().saturating_sub(end_trim).max(1.min(source.len()))];

    // match TypeScript's normalization of physical line endings in templates, leaving written \r and \n escapes
    // intact; normalize before removing line continuations so CR, CRLF, and LF all use the same check
    let text = text.replace("\r\n", "\n").replace('\r', "\n");

    // emit one Unicode code point per surrogate pair for Luau, e.g. 😀 becomes \u{1f600}; combine high
    // and low surrogates before converting individual \uXXXX escapes, consuming escaped backslashes first so literal
    // text like \\uD83D\\uDE00 stays untouched
    let bytes = text.as_bytes();
    let mut combined = String::with_capacity(text.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            if bytes.get(i + 1) == Some(&b'\\') {
                combined.push_str("\\\\");
                i += 2;
                continue;
            }
            if let Some((high, low)) = surrogate_pair_at(bytes, i + 1) {
                let code_point = char::decode_utf16([high, low]).next().and_then(Result::ok).map_or(0, u32::from);
                combined.push_str(&format!("\\u{{{code_point:x}}}"));
                i += 12;
                continue;
            }
        }
        let c = text[i..].chars().next().expect("in bounds");
        combined.push(c);
        i += c.len_utf8();
    }

    // consume whole escapes so \\u0041 stays literal text instead of matching a Unicode escape at the second
    // backslash; any character (line breaks included) can follow, so the same pass removes line continuations
    let is_string_literal = node.kind() == Kind::StringLiteral;
    let bytes = combined.as_bytes();
    let mut out = String::with_capacity(combined.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' || i + 1 >= bytes.len() {
            let c = combined[i..].chars().next().expect("in bounds");
            out.push(c);
            i += c.len_utf8();
            continue;
        }
        // add the braces Luau requires for Unicode escapes, preserving \b as \u{005c}b
        if bytes[i + 1] == b'u' && bytes.len() >= i + 6 && bytes[i + 2..i + 6].iter().all(|&c| is_hex(c)) {
            out.push_str("\\u{");
            out.push_str(&combined[i + 2..i + 6]);
            out.push('}');
            i += 6;
            continue;
        }
        let character = combined[i + 1..].chars().next().expect("in bounds");
        let escape_len = 1 + character.len_utf8();
        if matches!(character, '\n' | '\u{2028}' | '\u{2029}') {
            // a backslash followed by LF, a line separator or a paragraph separator adds no characters
        } else if "0bfnrtvux\\\"'".contains(character) || (character == '`' && !is_string_literal) {
            // shared escapes stay for Luau to interpret; backticks stay escaped in templates, whose Luau form also
            // uses backtick delimiters
            out.push_str(&combined[i..i + escape_len]);
        } else {
            // identity escapes lose their backslash, as in TypeScript (Luau's \a is a bell and \z skips whitespace)
            out.push(character);
        }
        i += escape_len;
    }
    // rbxts-rs: fixes roblox-ts copying a raw NUL (written in the source, or after a backslash) into the output, which
    // Luau rejects in every kind of string ("Malformed string"). `\x00` takes exactly two digits, so a following
    // digit stays a digit. Escapes stay escapes: a NUL character here is always a NUL in the value.
    if out.contains('\0') { out.replace('\0', "\\x00") } else { out }
}

/// rbxts-rs: a Luau string literal whose value is exactly `value` (plain text, not source text). roblox-ts passes such
/// values to `luau.string`, whose renderer treats them as source text between delimiters. Text that renders as
/// itself keeps that spelling; other text is escaped ([`rbxts_plugin::text_literal`]): text with a NUL (malformed in
/// every Luau string) or a carriage return (malformed between quotes; a long bracket reads CR LF as LF), text the
/// renderer puts between quotes with a backslash (an escape there), and text it puts in a long bracket with a leading
/// newline (`[[` drops a newline right after it). A long bracket reads a backslash as itself.
pub fn create_string_from_value(value: &str) -> rbxts_luau::Expr {
    let long_bracket = value.contains('\n') || (value.contains('"') && value.contains('\''));
    let renders_as_itself = !value.contains(['\r', '\0'])
        && if long_bracket { !value.starts_with('\n') } else { !value.contains('\\') };
    if renders_as_itself { rbxts_luau::create::string(value) } else { rbxts_plugin::text_literal(value) }
}
