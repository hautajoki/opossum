//! `util/fixupWhitespaceAndDecodeEntities.ts`: TypeScript's JSX text rule (from its JSX transformer). Lines are
//! trimmed, except that the start and end of the text count as line ends only on lines of their own; empty lines are
//! dropped, the rest are joined with a space, and HTML entities are decoded per line.
//!
//! rbxts-rs: the text is processed as UTF-16 code units, as in TypeScript, so that indexes, entities that encode
//! surrogates and lone surrogates behave the same. The result is converted back lossily, as Node writes a lone
//! surrogate as U+FFFD.

use tsrs_core::stringutil::{is_line_break, is_white_space_single_line};

/// An entity decodes to a code point above U+10FFFF, where roblox-ts throws (`String.fromCodePoint`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidCodePoint;

/// `fixupWhitespaceAndDecodeEntities`: JSX trims whitespace at the end and beginning of lines, except that the
/// start/end of a tag is considered a start/end of a line only if that line is on the same line as the closing tag.
/// `None` when the text is only whitespace over several lines.
pub fn fixup_whitespace_and_decode_entities(text: &str) -> Result<Option<String>, InvalidCodePoint> {
    let text: Vec<u16> = text.encode_utf16().collect();
    let mut acc: Option<Vec<u16>> = None;
    // First non-whitespace character on this line.
    let mut first_non_whitespace: Option<usize> = Some(0);
    // Last non-whitespace character on this line.
    let mut last_non_whitespace: Option<usize> = None;
    // These initial values are special because the first line is: first_non_whitespace = 0 to indicate that we want
    // leading whitespace, but last_non_whitespace = None as a special flag to indicate that we *don't* include the
    // line if it's all whitespace.

    for (i, &c) in text.iter().enumerate() {
        if is_line_break(u32::from(c)) {
            // If we've seen any non-whitespace characters on this line, add the 'trim' of the line.
            if let (Some(first), Some(last)) = (first_non_whitespace, last_non_whitespace) {
                acc = Some(add_line_of_jsx_text(acc, &text[first..=last])?);
            }
            // Reset first_non_whitespace for the next line. Don't bother to reset last_non_whitespace because we
            // ignore it if first_non_whitespace is None.
            first_non_whitespace = None;
        } else if !is_white_space_single_line(u32::from(c)) {
            last_non_whitespace = Some(i);
            if first_non_whitespace.is_none() {
                first_non_whitespace = Some(i);
            }
        }
    }

    let result = match first_non_whitespace {
        // Last line had a non-whitespace character. Emit the 'trimLeft', meaning keep trailing whitespace.
        Some(first) => Some(add_line_of_jsx_text(acc, &text[first..])?),
        // Last line was all whitespace, so ignore it
        None => acc,
    };
    Ok(result.map(|units| String::from_utf16_lossy(&units)))
}

fn add_line_of_jsx_text(acc: Option<Vec<u16>>, trimmed_line: &[u16]) -> Result<Vec<u16>, InvalidCodePoint> {
    // We do not escape the string here as that is handled by the printer when it emits the literal. We do, however,
    // need to decode JSX entities.
    let decoded = decode_entities(trimmed_line)?;
    Ok(match acc {
        None => decoded,
        Some(mut acc) => {
            acc.push(u16::from(b' '));
            acc.extend(decoded);
            acc
        }
    })
}

/// The length of the run of ASCII units from `start` that pass `test` (`\d`, `[\da-fA-F]` and `\w` are ASCII only).
fn run_length(text: &[u16], start: usize, test: fn(&u8) -> bool) -> usize {
    text.get(start..).map_or(0, |rest| rest.iter().take_while(|&&c| u8::try_from(c).is_ok_and(|c| test(&c))).count())
}

fn is_word(c: &u8) -> bool {
    c.is_ascii_alphanumeric() || *c == b'_'
}

/// The value of ASCII digits in `radix`, saturating above the last code point.
fn parse_code_point(digits: &[u16], radix: u32) -> u32 {
    digits.iter().fold(0u32, |value, &c| {
        let digit = char::from_u32(u32::from(c)).and_then(|c| c.to_digit(radix)).expect("a digit");
        value.saturating_mul(radix).saturating_add(digit).min(0x11_0000)
    })
}

/// `utf16EncodeAsString` (`String.fromCodePoint`)
fn utf16_encode(code_point: u32, out: &mut Vec<u16>) -> Result<(), InvalidCodePoint> {
    if code_point > 0x10_FFFF {
        return Err(InvalidCodePoint);
    }
    if let Ok(unit) = u16::try_from(code_point) {
        out.push(unit);
    } else {
        let offset = code_point - 0x1_0000;
        let [high, low] = [0xD800 + offset / 0x400, 0xDC00 + offset % 0x400].map(|u| u16::try_from(u).expect("a unit"));
        out.extend([high, low]);
    }
    Ok(())
}

/// The entity at `text[i]` (an `&`): the code point it decodes to (`None` for an unknown name, which stays as
/// written) and the length it spans. This is TypeScript's regex `&((#((\d+)|x([\da-fA-F]+)))|(\w+));`, whose greedy
/// runs never need to backtrack (none can end in `;`).
fn match_entity(text: &[u16], i: usize) -> Option<(Option<u32>, usize)> {
    let semicolon_at = |end: usize| text.get(end) == Some(&u16::from(b';'));
    if text.get(i + 1) == Some(&u16::from(b'#')) {
        let decimal = run_length(text, i + 2, u8::is_ascii_digit);
        if decimal > 0 && semicolon_at(i + 2 + decimal) {
            return Some((Some(parse_code_point(&text[i + 2..i + 2 + decimal], 10)), decimal + 3));
        }
        if text.get(i + 2) == Some(&u16::from(b'x')) {
            let hex = run_length(text, i + 3, u8::is_ascii_hexdigit);
            if hex > 0 && semicolon_at(i + 3 + hex) {
                return Some((Some(parse_code_point(&text[i + 3..i + 3 + hex], 16)), hex + 4));
            }
        }
        return None;
    }
    let word = run_length(text, i + 1, is_word);
    if word > 0 && semicolon_at(i + 1 + word) {
        let name = String::from_utf16_lossy(&text[i + 1..i + 1 + word]);
        let code_point =
            ENTITIES.binary_search_by(|(entity, _)| (*entity).cmp(name.as_str())).ok().map(|k| ENTITIES[k].1);
        return Some((code_point, word + 2));
    }
    None
}

/// `decodeEntities`: replaces entities like `&nbsp;`, `&#123;` and `&#xDEADBEEF;` with the characters they encode.
/// See <https://en.wikipedia.org/wiki/List_of_XML_and_HTML_character_entity_references>
fn decode_entities(text: &[u16]) -> Result<Vec<u16>, InvalidCodePoint> {
    let mut out = Vec::with_capacity(text.len());
    let mut i = 0;
    while i < text.len() {
        if text[i] == u16::from(b'&')
            && let Some((code_point, length)) = match_entity(text, i)
        {
            match code_point {
                Some(code_point) => utf16_encode(code_point, &mut out)?,
                // If this is not a valid entity, then just use the match (replace it with itself, i.e. don't replace)
                None => out.extend_from_slice(&text[i..i + length]),
            }
            i += length;
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
    Ok(out)
}

/// rbxts-rs: `decodeEntities` on a whole string, as TypeScript's JSX transform decodes an attribute's string value
/// (`tryDecodeEntities`).
pub fn decode_entities_in_text(text: &str) -> Result<String, InvalidCodePoint> {
    let units: Vec<u16> = text.encode_utf16().collect();
    Ok(String::from_utf16_lossy(&decode_entities(&units)?))
}

/// `entities`: the named entities TypeScript decodes, sorted by name for a binary search.
#[rustfmt::skip]
const ENTITIES: &[(&str, u32)] = &[
    ("AElig", 0x00c6), ("Aacute", 0x00c1), ("Acirc", 0x00c2), ("Agrave", 0x00c0), ("Alpha", 0x0391),
    ("Aring", 0x00c5), ("Atilde", 0x00c3), ("Auml", 0x00c4), ("Beta", 0x0392), ("Ccedil", 0x00c7), ("Chi", 0x03a7),
    ("Dagger", 0x2021), ("Delta", 0x0394), ("ETH", 0x00d0), ("Eacute", 0x00c9), ("Ecirc", 0x00ca),
    ("Egrave", 0x00c8), ("Epsilon", 0x0395), ("Eta", 0x0397), ("Euml", 0x00cb), ("Gamma", 0x0393),
    ("Iacute", 0x00cd), ("Icirc", 0x00ce), ("Igrave", 0x00cc), ("Iota", 0x0399), ("Iuml", 0x00cf),
    ("Kappa", 0x039a), ("Lambda", 0x039b), ("Mu", 0x039c), ("Ntilde", 0x00d1), ("Nu", 0x039d), ("OElig", 0x0152),
    ("Oacute", 0x00d3), ("Ocirc", 0x00d4), ("Ograve", 0x00d2), ("Omega", 0x03a9), ("Omicron", 0x039f),
    ("Oslash", 0x00d8), ("Otilde", 0x00d5), ("Ouml", 0x00d6), ("Phi", 0x03a6), ("Pi", 0x03a0), ("Prime", 0x2033),
    ("Psi", 0x03a8), ("Rho", 0x03a1), ("Scaron", 0x0160), ("Sigma", 0x03a3), ("THORN", 0x00de), ("Tau", 0x03a4),
    ("Theta", 0x0398), ("Uacute", 0x00da), ("Ucirc", 0x00db), ("Ugrave", 0x00d9), ("Upsilon", 0x03a5),
    ("Uuml", 0x00dc), ("Xi", 0x039e), ("Yacute", 0x00dd), ("Yuml", 0x0178), ("Zeta", 0x0396), ("aacute", 0x00e1),
    ("acirc", 0x00e2), ("acute", 0x00b4), ("aelig", 0x00e6), ("agrave", 0x00e0), ("alefsym", 0x2135),
    ("alpha", 0x03b1), ("amp", 0x0026), ("and", 0x2227), ("ang", 0x2220), ("apos", 0x0027), ("aring", 0x00e5),
    ("asymp", 0x2248), ("atilde", 0x00e3), ("auml", 0x00e4), ("bdquo", 0x201e), ("beta", 0x03b2),
    ("brvbar", 0x00a6), ("bull", 0x2022), ("cap", 0x2229), ("ccedil", 0x00e7), ("cedil", 0x00b8), ("cent", 0x00a2),
    ("chi", 0x03c7), ("circ", 0x02c6), ("clubs", 0x2663), ("cong", 0x2245), ("copy", 0x00a9), ("crarr", 0x21b5),
    ("cup", 0x222a), ("curren", 0x00a4), ("dArr", 0x21d3), ("dagger", 0x2020), ("darr", 0x2193), ("deg", 0x00b0),
    ("delta", 0x03b4), ("diams", 0x2666), ("divide", 0x00f7), ("eacute", 0x00e9), ("ecirc", 0x00ea),
    ("egrave", 0x00e8), ("empty", 0x2205), ("emsp", 0x2003), ("ensp", 0x2002), ("epsilon", 0x03b5),
    ("equiv", 0x2261), ("eta", 0x03b7), ("eth", 0x00f0), ("euml", 0x00eb), ("euro", 0x20ac), ("exist", 0x2203),
    ("fnof", 0x0192), ("forall", 0x2200), ("frac12", 0x00bd), ("frac14", 0x00bc), ("frac34", 0x00be),
    ("frasl", 0x2044), ("gamma", 0x03b3), ("ge", 0x2265), ("gt", 0x003e), ("hArr", 0x21d4), ("harr", 0x2194),
    ("hearts", 0x2665), ("hellip", 0x2026), ("iacute", 0x00ed), ("icirc", 0x00ee), ("iexcl", 0x00a1),
    ("igrave", 0x00ec), ("image", 0x2111), ("infin", 0x221e), ("int", 0x222b), ("iota", 0x03b9), ("iquest", 0x00bf),
    ("isin", 0x2208), ("iuml", 0x00ef), ("kappa", 0x03ba), ("lArr", 0x21d0), ("lambda", 0x03bb), ("lang", 0x2329),
    ("laquo", 0x00ab), ("larr", 0x2190), ("lceil", 0x2308), ("ldquo", 0x201c), ("le", 0x2264), ("lfloor", 0x230a),
    ("lowast", 0x2217), ("loz", 0x25ca), ("lrm", 0x200e), ("lsaquo", 0x2039), ("lsquo", 0x2018), ("lt", 0x003c),
    ("macr", 0x00af), ("mdash", 0x2014), ("micro", 0x00b5), ("middot", 0x00b7), ("minus", 0x2212), ("mu", 0x03bc),
    ("nabla", 0x2207), ("nbsp", 0x00a0), ("ndash", 0x2013), ("ne", 0x2260), ("ni", 0x220b), ("not", 0x00ac),
    ("notin", 0x2209), ("nsub", 0x2284), ("ntilde", 0x00f1), ("nu", 0x03bd), ("oacute", 0x00f3), ("ocirc", 0x00f4),
    ("oelig", 0x0153), ("ograve", 0x00f2), ("oline", 0x203e), ("omega", 0x03c9), ("omicron", 0x03bf),
    ("oplus", 0x2295), ("or", 0x2228), ("ordf", 0x00aa), ("ordm", 0x00ba), ("oslash", 0x00f8), ("otilde", 0x00f5),
    ("otimes", 0x2297), ("ouml", 0x00f6), ("para", 0x00b6), ("part", 0x2202), ("permil", 0x2030), ("perp", 0x22a5),
    ("phi", 0x03c6), ("pi", 0x03c0), ("piv", 0x03d6), ("plusmn", 0x00b1), ("pound", 0x00a3), ("prime", 0x2032),
    ("prod", 0x220f), ("prop", 0x221d), ("psi", 0x03c8), ("quot", 0x0022), ("rArr", 0x21d2), ("radic", 0x221a),
    ("rang", 0x232a), ("raquo", 0x00bb), ("rarr", 0x2192), ("rceil", 0x2309), ("rdquo", 0x201d), ("real", 0x211c),
    ("reg", 0x00ae), ("rfloor", 0x230b), ("rho", 0x03c1), ("rlm", 0x200f), ("rsaquo", 0x203a), ("rsquo", 0x2019),
    ("sbquo", 0x201a), ("scaron", 0x0161), ("sdot", 0x22c5), ("sect", 0x00a7), ("shy", 0x00ad), ("sigma", 0x03c3),
    ("sigmaf", 0x03c2), ("sim", 0x223c), ("spades", 0x2660), ("sub", 0x2282), ("sube", 0x2286), ("sum", 0x2211),
    ("sup", 0x2283), ("sup1", 0x00b9), ("sup2", 0x00b2), ("sup3", 0x00b3), ("supe", 0x2287), ("szlig", 0x00df),
    ("tau", 0x03c4), ("there4", 0x2234), ("theta", 0x03b8), ("thetasym", 0x03d1), ("thinsp", 0x2009),
    ("thorn", 0x00fe), ("tilde", 0x02dc), ("times", 0x00d7), ("trade", 0x2122), ("uArr", 0x21d1),
    ("uacute", 0x00fa), ("uarr", 0x2191), ("ucirc", 0x00fb), ("ugrave", 0x00f9), ("uml", 0x00a8), ("upsih", 0x03d2),
    ("upsilon", 0x03c5), ("uuml", 0x00fc), ("weierp", 0x2118), ("xi", 0x03be), ("yacute", 0x00fd), ("yen", 0x00a5),
    ("yuml", 0x00ff), ("zeta", 0x03b6), ("zwj", 0x200d), ("zwnj", 0x200c),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn fixup(text: &str) -> Option<String> {
        fixup_whitespace_and_decode_entities(text).unwrap()
    }

    #[test]
    fn entities_are_sorted() {
        assert_eq!(ENTITIES.len(), 253);
        assert!(ENTITIES.windows(2).all(|w| w[0].0 < w[1].0));
    }

    #[test]
    fn whitespace() {
        assert_eq!(fixup("  a  ").as_deref(), Some("  a  "));
        assert_eq!(fixup("\n  \n  "), None);
        assert_eq!(fixup(" \u{a0}\n\t\t"), None);
        assert_eq!(fixup("value\n\n\t").as_deref(), Some("value"));
        assert_eq!(fixup("\n\t a b \n\t c  \n").as_deref(), Some("a b c"));
        assert_eq!(fixup(" a \n b ").as_deref(), Some(" a b "));
        assert_eq!(fixup("\r\n x\u{2028}y").as_deref(), Some("x y"));
    }

    #[test]
    fn entities() {
        assert_eq!(
            fixup("&amp;&lt;&gt;&quot;&apos;&#65;&#x42;&unknown;\\path").as_deref(),
            Some("&<>\"'AB&unknown;\\path")
        );
        assert_eq!(
            fixup("&#X41; &#65 &#x; &; &#x1F600; &nbsp;").as_deref(),
            Some("&#X41; &#65 &#x; &; \u{1F600} \u{a0}")
        );
        assert_eq!(fixup("&#xD83D;&#xDE00;").as_deref(), Some("\u{1F600}"));
        assert_eq!(fixup("&#xD83D;x").as_deref(), Some("\u{FFFD}x"));
        assert_eq!(fixup("&&amp;;").as_deref(), Some("&&;"));
        assert_eq!(fixup_whitespace_and_decode_entities("&#x110000;"), Err(InvalidCodePoint));
        assert_eq!(fixup_whitespace_and_decode_entities("&#99999999999999999999;"), Err(InvalidCodePoint));
    }
}
