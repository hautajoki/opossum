//! Name and number rules (`LuauAST/util`), and the JavaScript number conversions the renderer and the transformer
//! need to produce the same text as roblox-ts.

/// Luau's reserved keywords (`LUAU_RESERVED_KEYWORDS`).
pub const LUAU_RESERVED_KEYWORDS: [&str; 21] = [
    "and", "break", "do", "else", "elseif", "end", "false", "for", "function", "if", "in", "local", "nil", "not", "or",
    "repeat", "return", "then", "true", "until", "while",
];

/// Luau's metamethod names.
pub const LUAU_METAMETHODS: [&str; 19] = [
    "__index",
    "__newindex",
    "__call",
    "__concat",
    "__unm",
    "__add",
    "__sub",
    "__mul",
    "__div",
    "__mod",
    "__pow",
    "__tostring",
    "__metatable",
    "__eq",
    "__lt",
    "__le",
    "__mode",
    "__gc",
    "__len",
];

/// Class fields roblox-ts reserves for its class implementation.
pub const LUAU_RESERVED_CLASS_FIELDS: [&str; 2] = ["__index", "new"];

/// The names of `luau.globals` (`isReservedIdentifier`).
pub const RESERVED_IDENTIFIERS: [&str; 27] = [
    "_G",
    "TS",
    "assert",
    "bit32",
    "coroutine",
    "error",
    "exports",
    "getmetatable",
    "ipairs",
    "next",
    "pairs",
    "pcall",
    "require",
    "script",
    "select",
    "self",
    "setmetatable",
    "string",
    "super",
    "table",
    "utf8",
    "math",
    "tostring",
    "type",
    "typeof",
    "unpack",
    "game",
];

/// True when `id` matches `^[A-Za-z_][A-Za-z0-9_]*$` and is not a reserved keyword.
pub fn is_valid_identifier(id: &str) -> bool {
    let bytes = id.as_bytes();
    let Some(&first) = bytes.first() else { return false };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes[1..].iter().all(|&b| b.is_ascii_alphanumeric() || b == b'_')
        && !LUAU_RESERVED_KEYWORDS.contains(&id)
}

/// True for Luau's metamethod names (`__index`, `__add`, …).
pub fn is_metamethod(id: &str) -> bool {
    LUAU_METAMETHODS.contains(&id)
}

/// True for `__index` and `new`, which roblox-ts's classes use themselves.
pub fn is_reserved_class_field(id: &str) -> bool {
    LUAU_RESERVED_CLASS_FIELDS.contains(&id)
}

/// True for the names in `luau.globals` (`TS`, `script`, `string`, …), which user code must not shadow.
pub fn is_reserved_identifier(id: &str) -> bool {
    RESERVED_IDENTIFIERS.contains(&id)
}

/// True when `text` is a Luau number literal: decimal (with `.`, exponent), `0x` hex or `0b` binary, underscores
/// allowed as in Luau.
pub fn is_valid_number_literal(text: &str) -> bool {
    is_decimal_literal(text.as_bytes())
        || is_prefixed_literal(text.as_bytes(), b'b', |b| matches!(b, b'0' | b'1'))
        || is_prefixed_literal(text.as_bytes(), b'x', |b| b.is_ascii_hexdigit())
}

/// `^(?:\d[\d_]*(?:\.[\d_]*)?|\.\d[\d_]*)(?:[eE][+-]?_*\d[\d_]*)?$`
fn is_decimal_literal(s: &[u8]) -> bool {
    let digits = |s: &[u8], mut i: usize| {
        while i < s.len() && (s[i].is_ascii_digit() || s[i] == b'_') {
            i += 1;
        }
        i
    };
    let mut i;
    match s.first() {
        Some(b) if b.is_ascii_digit() => {
            i = digits(s, 1);
            if s.get(i) == Some(&b'.') {
                i = digits(s, i + 1);
            }
        }
        Some(b'.') if s.get(1).is_some_and(u8::is_ascii_digit) => i = digits(s, 2),
        _ => return false,
    }
    if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
        i += 1;
        if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
            i += 1;
        }
        while i < s.len() && s[i] == b'_' {
            i += 1;
        }
        if !s.get(i).is_some_and(u8::is_ascii_digit) {
            return false;
        }
        i = digits(s, i + 1);
    }
    i == s.len()
}

/// `^0_*[bB]_*[01][01_]*$` and `^0_*[xX]_*[\da-fA-F][\da-fA-F_]*$`
fn is_prefixed_literal(s: &[u8], prefix: u8, digit: fn(u8) -> bool) -> bool {
    if s.first() != Some(&b'0') {
        return false;
    }
    let mut i = 1;
    while s.get(i) == Some(&b'_') {
        i += 1;
    }
    if !s.get(i).is_some_and(|b| b.to_ascii_lowercase() == prefix) {
        return false;
    }
    i += 1;
    while s.get(i) == Some(&b'_') {
        i += 1;
    }
    if !s.get(i).copied().is_some_and(digit) {
        return false;
    }
    s[i + 1..].iter().all(|&b| digit(b) || b == b'_')
}

/// JavaScript's whitespace and line terminators, as `String.prototype.trim` and `Number(string)` strip them.
pub fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{0B}' | '\u{0C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}' | '\u{FEFF}'
    )
}

/// JavaScript's `Number(text)` (StringToNumber): whitespace trimmed, empty is 0, `0x` / `0o` / `0b` integers,
/// decimal with an optional sign, `Infinity`; anything else is NaN.
pub fn js_string_to_number(text: &str) -> f64 {
    let s = text.trim_matches(is_js_whitespace);
    if s.is_empty() {
        return 0.0;
    }
    let bytes = s.as_bytes();
    if bytes.len() > 2 && bytes[0] == b'0' {
        let radix = match bytes[1] {
            b'x' | b'X' => 16,
            b'o' | b'O' => 8,
            b'b' | b'B' => 2,
            _ => 0,
        };
        if radix != 0 {
            return parse_radix(&s[2..], radix);
        }
    }
    let (sign, unsigned) = match bytes[0] {
        b'+' => (1.0, &s[1..]),
        b'-' => (-1.0, &s[1..]),
        _ => (1.0, s),
    };
    if unsigned == "Infinity" {
        return sign * f64::INFINITY;
    }
    // Rust's parser also takes "inf", "nan" and friends, so the syntax is checked first.
    let u = unsigned.as_bytes();
    let int_end = u.iter().position(|b| !b.is_ascii_digit()).unwrap_or(u.len());
    let mut i = int_end;
    let mut frac_digits = 0;
    if u.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        while u.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        frac_digits = i - start;
    }
    if int_end == 0 && frac_digits == 0 {
        return f64::NAN;
    }
    if matches!(u.get(i), Some(b'e' | b'E')) {
        i += 1;
        if matches!(u.get(i), Some(b'+' | b'-')) {
            i += 1;
        }
        let start = i;
        while u.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return f64::NAN;
        }
    }
    if i != u.len() {
        return f64::NAN;
    }
    sign * unsigned.parse::<f64>().unwrap_or(f64::NAN)
}

fn parse_radix(digits: &str, radix: u32) -> f64 {
    if digits.is_empty() {
        return f64::NAN;
    }
    // Exact while it fits in a u128; beyond that the value is accumulated in f64, which can be off by an ulp from
    // JavaScript's correctly rounded result (literals above 2^128 do not occur in practice).
    let mut exact: u128 = 0;
    let mut approx: Option<f64> = None;
    for c in digits.chars() {
        let Some(d) = c.to_digit(radix) else { return f64::NAN };
        match approx {
            Some(ref mut a) => *a = *a * f64::from(radix) + f64::from(d),
            None => match exact.checked_mul(u128::from(radix)).and_then(|v| v.checked_add(u128::from(d))) {
                Some(v) => exact = v,
                None => approx = Some(exact as f64 * f64::from(radix) + f64::from(d)),
            },
        }
    }
    approx.unwrap_or(exact as f64)
}

/// JavaScript's `String(number)` (Number::toString): the shortest round-trip digits, in plain notation for
/// exponents -7 < e < 21 and as `1.5e+21` / `1e-7` otherwise; `NaN`, `Infinity`, `-Infinity`; `-0` is `0`.
pub fn js_number_to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value == 0.0 {
        return "0".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let mut out = String::new();
    if value < 0.0 {
        out.push('-');
    }
    // `{:e}` gives the shortest round-trip digits as d[.ddd]e<exp>, the same digits JavaScript picks.
    let sci = format!("{:e}", value.abs());
    let (mantissa, exp) = sci.split_once('e').expect("`{:e}` has an exponent");
    let exp: i32 = exp.parse().expect("`{:e}` exponent is an integer");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exp + 1; // the decimal point sits after `n` digits
    if k <= n && n <= 21 {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (n - k) as usize));
    } else if 0 < n && n <= 21 {
        out.push_str(&digits[..n as usize]);
        out.push('.');
        out.push_str(&digits[n as usize..]);
    } else if -6 < n && n <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-n) as usize));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if n > 0 { '+' } else { '-' });
        out.push_str(&(n - 1).abs().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers() {
        for ok in ["a", "_", "_a1", "A_b", "self", "TS"] {
            assert!(is_valid_identifier(ok), "{ok}");
        }
        for bad in ["", "1a", "a-b", "end", "nil", "local", "é", "a b"] {
            assert!(!is_valid_identifier(bad), "{bad}");
        }
        assert!(is_metamethod("__index") && !is_metamethod("__foo"));
        assert!(is_reserved_class_field("new") && is_reserved_identifier("TS") && !is_reserved_identifier("ts"));
    }

    #[test]
    fn number_literals() {
        for ok in ["0", "1_000", "1.", "1.5", ".5", "1e10", "1E+1_0", "1e_5", "0x1F", "0_x_f", "0b1010", "0B_1", "5._"]
        {
            assert!(is_valid_number_literal(ok), "{ok}");
        }
        for bad in ["", ".", "_1", "1e", "0x", "0b2", "0o17", "Infinity", "NaN", "1n", "-1", "1e+", ".e1", "0xg"] {
            assert!(!is_valid_number_literal(bad), "{bad}");
        }
    }

    #[test]
    fn string_to_number() {
        let cases: [(&str, f64); 14] = [
            ("", 0.0),
            (" 12 ", 12.0),
            ("0o17", 15.0),
            ("0x1F", 31.0),
            ("0b101", 5.0),
            ("1.", 1.0),
            (".5", 0.5),
            ("-2e3", -2000.0),
            ("+Infinity", f64::INFINITY),
            ("-Infinity", f64::NEG_INFINITY),
            ("1.0", 1.0),
            ("0x", f64::NAN),
            ("1_0", f64::NAN),
            ("inf", f64::NAN),
        ];
        for (text, expected) in cases {
            let got = js_string_to_number(text);
            assert!(got == expected || (got.is_nan() && expected.is_nan()), "{text}: {got} != {expected}");
        }
        assert!(js_string_to_number("nan").is_nan() && js_string_to_number("-0x5").is_nan());
    }

    #[test]
    fn number_to_string() {
        let cases: [(f64, &str); 16] = [
            (0.0, "0"),
            (-0.0, "0"),
            (1.0, "1"),
            (-1.5, "-1.5"),
            (0.1 + 0.2, "0.30000000000000004"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (123456789012345680000.0, "123456789012345680000"),
            (1e-6, "0.000001"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (2.5e300, "2.5e+300"),
            (f64::NAN, "NaN"),
            (f64::INFINITY, "Infinity"),
            (f64::NEG_INFINITY, "-Infinity"),
            (4294967296.5, "4294967296.5"),
        ];
        for (value, expected) in cases {
            assert_eq!(js_number_to_string(value), expected, "{value:e}");
        }
    }
}
