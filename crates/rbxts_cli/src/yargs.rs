//! roblox-ts's command line as yargs parses it: a port of the parts of yargs 18.2.0 and its yargs-parser 22.0.0
//! that roblox-ts's `rbxtsc` reaches (`src/CLI/cli.ts`: `.strict()`, help, version, the `build` command as the default
//! command; `CLI/commands/build.ts`: its options), so that every argument vector is accepted, refused and read as
//! stock reads it.
//!
//! What is ported, rule for rule:
//! - **yargs-parser's argument loop**: `--key=value`, `--no-key` (negation), `--key [value]`, dot-notation keys, short
//!   option groups (`-wp x`, `-p=x`, `-p-`), `--` ending the options, `---x`;
//! - **values**: a string option takes the next argument unless it starts with `-` (a negative number excepted), a
//!   boolean only a following `true` / `false`; a boolean's value other than `"true"` is false (`--luau=yes`); an
//!   option without a value takes its default (`project`'s `"."`), else `""` (string) or `true`; `--k="v"` loses its
//!   quotes; numbers are read in untyped options (`--type 5`); a repeated non-boolean option collects an array, a
//!   repeated boolean keeps the last value;
//! - **aliases**: the declared ones (`-p`, `-w`, `-i`, `-h`, `-v`), every declared name's kebab-case form
//!   (`--include-path`), and the camel-case form of any dashed key (`--INCLUDE-PATH` is `includePath`), with
//!   yargs-parser's "new alias" bookkeeping, which decides what strict mode calls unknown;
//! - **yargs**: the two parses (the top level, where only help and version are known, picks the command from its
//!   first positional; the command's own parse reads the options), `help` as the last positional, the help and
//!   version flags, then strict mode's checks in yargs's order: unknown arguments (option keys in JavaScript's key
//!   order, then the positionals after the command), `type`'s choices, `usePolling`'s implication of `watch`. Their
//!   messages are yargs's English ones.
//!
//! Not ported (no roblox-ts option uses them): arrays, `nargs`, counts, configuration files, environment variables.

use std::collections::{HashMap, HashSet};

/// A parsed value (a JavaScript value as yargs-parser leaves it).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// A boolean option, or a negated one (`--no-x`).
    Bool(bool),
    /// A string option's value.
    Str(String),
    /// A number: an untyped option whose value reads as one (`--checkers 4`).
    Num(f64),
    /// A repeated non-boolean option.
    Arr(Vec<Value>),
    /// A dot-notation key (`--project.x`): an object.
    Obj(Vec<(String, Value)>),
}

impl Value {
    /// `JSON.stringify`, as yargs prints invalid values.
    pub fn to_json(&self) -> String {
        match self {
            Value::Bool(b) => b.to_string(),
            Value::Str(s) => json_string(s),
            Value::Num(n) => js_number(*n),
            Value::Arr(items) => format!("[{}]", items.iter().map(Value::to_json).collect::<Vec<_>>().join(",")),
            Value::Obj(entries) => format!(
                "{{{}}}",
                entries
                    .iter()
                    .map(|(k, v)| format!("{}:{}", json_string(k), v.to_json()))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        }
    }
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// JavaScript's `Number#toString` for the numbers arguments can hold.
fn js_number(n: f64) -> String {
    if n == 0.0 {
        "0".to_owned()
    } else if n.fract() == 0.0 && n.abs() < 1e21 {
        format!("{n:.0}")
    } else {
        format!("{n}")
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bool,
    Str,
    /// Neither: `type` (it has only choices) and unknown keys.
    Untyped,
}

/// One declared option: name, kind, short alias, default.
struct Spec {
    name: &'static str,
    kind: Kind,
    alias: Option<&'static str>,
    default: Option<&'static str>,
}

const fn spec(name: &'static str, kind: Kind, alias: Option<&'static str>, default: Option<&'static str>) -> Spec {
    Spec { name, kind, alias, default }
}

/// roblox-ts's options (`build.ts`, plus yargs's `help` and `version`), and rbxts-rs's own: `--timings`,
/// `--events` with watch mode, and `--checkers` (untyped, so that yargs-parser reads its number as for `type`).
const BUILD_OPTIONS: &[Spec] = &[
    spec("help", Kind::Bool, Some("h"), None),
    spec("version", Kind::Bool, Some("v"), None),
    spec("project", Kind::Str, Some("p"), Some(".")),
    spec("watch", Kind::Bool, Some("w"), None),
    spec("usePolling", Kind::Bool, None, None),
    spec("verbose", Kind::Bool, None, None),
    spec("noInclude", Kind::Bool, None, None),
    spec("logTruthyChanges", Kind::Bool, None, None),
    spec("writeOnlyChanged", Kind::Bool, None, None),
    spec("writeTransformedFiles", Kind::Bool, None, None),
    spec("optimizedLoops", Kind::Bool, None, None),
    spec("type", Kind::Untyped, None, None),
    spec("includePath", Kind::Str, Some("i"), None),
    spec("rojo", Kind::Str, None, None),
    spec("allowCommentDirectives", Kind::Bool, None, None),
    spec("luau", Kind::Bool, None, None),
    spec("timings", Kind::Bool, None, None),
    spec("events", Kind::Str, None, None),
    spec("checkers", Kind::Untyped, None, None),
];

/// The top-level parse knows only yargs's own options.
const TOP_LEVEL_OPTIONS: &[Spec] =
    &[spec("help", Kind::Bool, Some("h"), None), spec("version", Kind::Bool, Some("v"), None)];

/// `type`'s choices.
pub const TYPE_CHOICES: &[&str] = &["game", "model", "package"];

/// The outcome of parsing roblox-ts's command line.
#[derive(Debug, PartialEq)]
pub enum Parsed {
    /// `--help`, `-h` or a last positional `help`.
    Help,
    /// `--version` or `-v`.
    Version,
    /// yargs's failure message (the CLI prints it and exits with 1).
    Fail(String),
    /// The options given or defaulted, by declared name.
    Options(Vec<(String, Value)>),
}

/// One yargs-parser run.
struct Parser {
    kinds: HashMap<String, Kind>,
    aliases: HashMap<String, Vec<String>>,
    new_aliases: HashSet<String>,
    defaults: Vec<(String, Value)>,
    argv: Vec<(String, Value)>,
    positionals: Vec<String>,
}

/// `/^-([0-9]+(\.[0-9]+)?|\.[0-9]+)$/`
fn is_negative(s: &str) -> bool {
    let Some(rest) = s.strip_prefix('-') else { return false };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    match rest.split_once('.') {
        None => digits(rest),
        Some((int, frac)) => (int.is_empty() || digits(int)) && digits(frac),
    }
}

/// yargs-parser's `camelCase`.
fn camel_case(s: &str) -> String {
    let is_camel_case = s != s.to_lowercase() && s != s.to_uppercase();
    let s = if is_camel_case { s.to_owned() } else { s.to_lowercase() };
    if !s.contains('-') && !s.contains('_') {
        return s;
    }
    let chars: Vec<char> = s.chars().collect();
    let start = chars.iter().take_while(|&&c| c == '-').count();
    let mut out = String::new();
    let mut next_upper = false;
    for (i, &c) in chars.iter().enumerate().skip(start) {
        let c = if next_upper {
            next_upper = false;
            c.to_uppercase().next().unwrap_or(c)
        } else {
            c
        };
        if i != 0 && (c == '-' || c == '_') {
            next_upper = true;
        } else if c != '-' && c != '_' {
            out.push(c);
        }
    }
    out
}

/// yargs-parser's `decamelize` with `-`.
fn decamelize(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        let lower = c.to_lowercase().next().unwrap_or(c);
        if lower != c && i > 0 {
            out.push('-');
            out.push(lower);
        } else {
            out.push(c);
        }
    }
    out
}

/// `looksLikeNumber` plus `maybeCoerceNumber`'s safe-integer check; the number `Number()` gives.
fn coerce_number(s: &str) -> Option<f64> {
    let lower = s.to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix("0x")
        && !hex.is_empty()
        && hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        // `parseFloat("0x…")` is 0, a safe integer
        return u64::from_str_radix(hex, 16).ok().map(|n| n as f64).or(Some(f64::INFINITY));
    }
    if s.len() > 1 && s.starts_with('0') && s.as_bytes()[1] != b'.' {
        return None;
    }
    // /^[-]?(?:\d+(?:\.\d*)?|\.\d+)(e[-+]?\d+)?$/
    let body = s.strip_prefix('-').unwrap_or(s);
    let (mantissa, exponent) = match body.find('e') {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok = match mantissa.split_once('.') {
        None => digits(mantissa),
        Some((int, frac)) => {
            (digits(int) && frac.bytes().all(|b| b.is_ascii_digit())) || (int.is_empty() && digits(frac))
        }
    };
    let exponent_ok = exponent.is_none_or(|e| digits(e.strip_prefix(['-', '+']).unwrap_or(e)));
    if !mantissa_ok || !exponent_ok {
        return None;
    }
    let n: f64 = s.parse().ok()?;
    // Number.isSafeInteger(Math.floor(parseFloat(value)))
    let floor = n.floor();
    (floor.is_finite() && floor.abs() <= 9_007_199_254_740_991.0).then_some(n)
}

/// `stripQuotes`
fn strip_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && (bytes[0] == b'\'' || bytes[0] == b'"') && bytes[bytes.len() - 1] == bytes[0] {
        &s[1..s.len() - 1]
    } else if bytes.len() == 1 && (bytes[0] == b'\'' || bytes[0] == b'"') {
        // `val[0] === val[val.length - 1]` holds for a lone quote too: `substring(1, 0)` is ""
        ""
    } else {
        s
    }
}

/// A raw value before `processValue`.
enum Raw {
    Text(String),
    Value(Value),
}

impl Parser {
    fn new(specs: &[Spec]) -> Parser {
        let mut parser = Parser {
            kinds: HashMap::new(),
            aliases: HashMap::new(),
            new_aliases: HashSet::new(),
            defaults: Vec::new(),
            argv: Vec::new(),
            positionals: Vec::new(),
        };
        for spec in specs {
            if spec.kind != Kind::Untyped {
                parser.kinds.insert(spec.name.to_owned(), spec.kind);
            }
        }
        // extendAliases
        for spec in specs {
            let key = spec.name.to_owned();
            if parser.aliases.contains_key(&key) {
                continue;
            }
            let mut list: Vec<String> = spec.alias.iter().map(|a| (*a).to_owned()).collect();
            for x in list.clone().into_iter().chain([key.clone()]) {
                if x.contains('-') {
                    let c = camel_case(&x);
                    if c != key && !list.contains(&c) {
                        parser.new_aliases.insert(c.clone());
                        list.push(c);
                    }
                }
            }
            for x in list.clone().into_iter().chain([key.clone()]) {
                if x.chars().count() > 1 && x.chars().any(|c| c.is_ascii_uppercase()) {
                    let c = decamelize(&x);
                    if c != key && !list.contains(&c) {
                        parser.new_aliases.insert(c.clone());
                        list.push(c);
                    }
                }
            }
            for x in &list {
                let others = std::iter::once(key.clone()).chain(list.iter().filter(|y| *y != x).cloned()).collect();
                parser.aliases.insert(x.clone(), others);
            }
            parser.aliases.insert(key.clone(), list);
        }
        // defaults, applied to every alias too
        for spec in specs {
            if let Some(default) = spec.default {
                parser.defaults.push((spec.name.to_owned(), Value::Str(default.to_owned())));
                for alias in parser.aliases.get(spec.name).cloned().unwrap_or_default() {
                    parser.defaults.push((alias, Value::Str(default.to_owned())));
                }
            }
        }
        parser
    }

    /// `checkAllAliases(key, flags.<kind>)`
    fn is(&self, key: &str, kind: Kind) -> bool {
        self.aliases
            .get(key)
            .into_iter()
            .flatten()
            .map(String::as_str)
            .chain([key])
            .any(|k| self.kinds.get(k) == Some(&kind))
    }

    fn default_value(&self, key: &str) -> Value {
        if !self.is(key, Kind::Bool)
            && let Some((_, value)) = self.defaults.iter().find(|(k, _)| k == key)
        {
            return value.clone();
        }
        if self.is(key, Kind::Str) { Value::Str(String::new()) } else { Value::Bool(true) }
    }

    fn add_new_alias(&mut self, key: &str, alias: &str) {
        if self.aliases.get(key).is_none_or(Vec::is_empty) {
            self.aliases.insert(key.to_owned(), vec![alias.to_owned()]);
            self.new_aliases.insert(alias.to_owned());
        }
        if self.aliases.get(alias).is_none_or(Vec::is_empty) {
            self.add_new_alias(alias, key);
        }
    }

    fn process_value(&self, key: &str, raw: Raw, strip: bool) -> Value {
        match raw {
            Raw::Value(value) => value,
            Raw::Text(text) => {
                let text = if strip { strip_quotes(&text).to_owned() } else { text };
                if self.is(key, Kind::Bool) {
                    Value::Bool(text == "true")
                } else if !self.is(key, Kind::Str)
                    && let Some(n) = coerce_number(&text)
                {
                    Value::Num(n)
                } else {
                    Value::Str(text)
                }
            }
        }
    }

    fn set_arg(&mut self, key: &str, raw: Raw, strip: bool) {
        if key.contains('-') {
            let alias = key.split('.').map(camel_case).collect::<Vec<_>>().join(".");
            self.add_new_alias(key, &alias);
        }
        let value = self.process_value(key, raw, strip);
        self.set_key(key, value.clone());
        for alias in self.aliases.get(key).cloned().unwrap_or_default() {
            self.set_key(&alias, value.clone());
        }
    }

    /// `setKey` with dot-notation (one level deep is what an argument can make here) and duplicate arrays.
    fn set_key(&mut self, key: &str, value: Value) {
        let (top, rest) = match key.split_once('.') {
            Some((top, rest)) => (top, Some(rest)),
            None => (key, None),
        };
        let value = match rest {
            Some(rest) => Value::Obj(vec![(rest.to_owned(), value)]),
            None => value,
        };
        let is_bool = self.is(key, Kind::Bool);
        match self.argv.iter_mut().find(|(k, _)| k == top) {
            None => self.argv.push((top.to_owned(), value)),
            Some((_, existing)) => match existing {
                Value::Arr(items) => items.push(value),
                Value::Obj(entries) if matches!(value, Value::Obj(_)) => {
                    if let Value::Obj(new) = value {
                        entries.extend(new);
                    }
                }
                _ if !is_bool => *existing = Value::Arr(vec![existing.clone(), value]),
                _ => *existing = value,
            },
        }
    }

    fn has_key(&self, key: &str) -> bool {
        self.argv.iter().any(|(k, _)| k == key)
    }

    /// yargs-parser's argument loop.
    fn parse(&mut self, args: &[String]) {
        let mut i = 0;
        while i < args.len() {
            let arg = args[i].as_str();
            let truncated =
                if arg.starts_with("---") { format!("---{}", arg.trim_start_matches('-')) } else { arg.to_owned() };
            if truncated.starts_with("---") && (truncated.len() == 3 || truncated.as_bytes()[3] == b'=') {
                self.positionals.push(arg.to_owned());
            } else if arg.starts_with("--") && arg.bytes().skip(3).any(|b| b == b'=') {
                // /^--.+=/, then m = /^--?([^=]+)=([\s\S]*)$/: `--key=value`, or `-` as the key of `--=…`
                let body = if arg.as_bytes()[2] == b'=' { &arg[1..] } else { &arg[2..] };
                let at = body.find('=').expect("an =");
                self.set_arg(&body[..at], Raw::Text(body[at + 1..].to_owned()), true);
            } else if let Some(key) = arg.strip_prefix("--no-").filter(|k| !k.is_empty()) {
                self.set_arg(key, Raw::Value(Value::Bool(false)), false);
            } else if arg.len() > 2 && arg.starts_with("--") {
                let key = &arg[2..];
                let next = args.get(i + 1);
                if let Some(next) = next
                    && (!next.starts_with('-') || is_negative(next))
                    && !self.is(key, Kind::Bool)
                {
                    self.set_arg(key, Raw::Text(next.clone()), false);
                    i += 1;
                } else if let Some(next) = next.filter(|n| *n == "true" || *n == "false") {
                    self.set_arg(key, Raw::Text(next.clone()), false);
                    i += 1;
                } else {
                    let value = self.default_value(key);
                    self.set_arg(key, Raw::Value(value), false);
                }
            } else if arg.len() > 4
                && arg.starts_with('-')
                && arg.as_bytes()[2] == b'.'
                && arg.bytes().skip(4).any(|b| b == b'=')
            {
                // dot-notation flag separated by '=': /^-([^=]+)=([\s\S]*)$/
                let body = &arg[1..];
                let at = body.find('=').expect("an =");
                self.set_arg(&body[..at], Raw::Text(body[at + 1..].to_owned()), false);
            } else if arg.len() > 3 && arg.starts_with('-') && arg.as_bytes()[2] == b'.' && !is_negative(arg) {
                let key = &arg[1..];
                let next = args.get(i + 1);
                if let Some(next) = next
                    && !next.starts_with('-')
                    && !self.is(key, Kind::Bool)
                {
                    self.set_arg(key, Raw::Text(next.clone()), false);
                    i += 1;
                } else {
                    let value = self.default_value(key);
                    self.set_arg(key, Raw::Value(value), false);
                }
            } else if arg.len() > 1 && arg.starts_with('-') && !arg[1..].starts_with('-') && !is_negative(arg) {
                i = self.short_group(args, i);
            } else if arg == "--" {
                break;
            } else {
                self.positionals.push(arg.to_owned());
            }
            i += 1;
        }
        // applyDefaultsAndAliases
        for (key, value) in self.defaults.clone() {
            if !self.has_key(&key) {
                self.set_key(&key, value.clone());
                for alias in self.aliases.get(&key).cloned().unwrap_or_default() {
                    if !self.has_key(&alias) {
                        self.set_key(&alias, value.clone());
                    }
                }
            }
        }
    }

    /// A short option group (`-abc`, `-p=x`, `-p-`): every letter but the last is a flag unless what follows makes
    /// it take a value; the last takes the next argument as a `--key` would.
    fn short_group(&mut self, args: &[String], mut i: usize) -> usize {
        let arg: Vec<char> = args[i].chars().collect();
        let letters = &arg[1..arg.len() - 1];
        let rest = |from: usize| arg[from..].iter().collect::<String>();
        let mut broken = false;
        for j in 0..letters.len() {
            let key = letters[j].to_string();
            let next = rest(j + 2);
            if letters.get(j + 1) == Some(&'=') {
                self.set_arg(&key, Raw::Text(rest(j + 3)), false);
                broken = true;
                break;
            }
            if next == "-" {
                self.set_arg(&key, Raw::Text(next), false);
                continue;
            }
            if letters[j].is_ascii_alphabetic() && coerce_number_shape(&next) && !self.is(&next, Kind::Bool) {
                self.set_arg(&key, Raw::Text(next), false);
                broken = true;
                break;
            }
            if letters.get(j + 1).is_some_and(|c| !(c.is_ascii_alphanumeric() || *c == '_')) {
                self.set_arg(&key, Raw::Text(next), false);
                broken = true;
                break;
            }
            let value = self.default_value(&key);
            self.set_arg(&key, Raw::Value(value), false);
        }
        let key = arg[arg.len() - 1].to_string();
        if !broken && key != "-" {
            let next = args.get(i + 1);
            let flag_like = |n: &str| {
                let b = n.as_bytes();
                (b.len() >= 2 && b[0] == b'-' && b[1] != b'-') || (b.len() >= 3 && b.starts_with(b"--") && b[2] != b'-')
            };
            if let Some(next) = next
                && (!flag_like(next) || is_negative(next))
                && !self.is(&key, Kind::Bool)
            {
                self.set_arg(&key, Raw::Text(next.clone()), false);
                i += 1;
            } else if let Some(next) = next.filter(|n| *n == "true" || *n == "false") {
                self.set_arg(&key, Raw::Text(next.clone()), false);
                i += 1;
            } else {
                let value = self.default_value(&key);
                self.set_arg(&key, Raw::Value(value), false);
            }
        }
        i
    }

    /// `isValidAndSomeAliasIsNotNew`
    fn is_known(&self, key: &str) -> bool {
        let Some(aliases) = self.aliases.get(key) else { return false };
        std::iter::once(key)
            .chain(aliases.iter().map(String::as_str))
            .any(|a| !self.new_aliases.contains(a) || !self.new_aliases.contains(key))
    }

    /// The keys in JavaScript's order: integer-like keys first, ascending, then the others as inserted.
    fn js_key_order(&self) -> Vec<&str> {
        let is_index = |k: &str| {
            k == "0" || (!k.starts_with('0') && !k.is_empty() && k.len() < 10 && k.bytes().all(|b| b.is_ascii_digit()))
        };
        let mut indices: Vec<&str> = self.argv.iter().map(|(k, _)| k.as_str()).filter(|k| is_index(k)).collect();
        indices.sort_by_key(|k| k.parse::<u64>().unwrap_or(0));
        indices.extend(self.argv.iter().map(|(k, _)| k.as_str()).filter(|k| !is_index(k)));
        indices
    }

    fn get(&self, key: &str) -> Option<&Value> {
        self.argv.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
}

/// `/^-?\d+(\.\d*)?(e-?\d+)?$/`, a short group's numeric value.
fn coerce_number_shape(s: &str) -> bool {
    let body = s.strip_prefix('-').unwrap_or(s);
    let (mantissa, exponent) = match body.find('e') {
        Some(i) => (&body[..i], Some(&body[i + 1..])),
        None => (body, None),
    };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok = match mantissa.split_once('.') {
        None => digits(mantissa),
        Some((int, frac)) => digits(int) && frac.bytes().all(|b| b.is_ascii_digit()),
    };
    mantissa_ok && exponent.is_none_or(|e| digits(e.strip_prefix('-').unwrap_or(e)))
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Str(s)) => !s.is_empty(),
        Some(Value::Num(n)) => *n != 0.0 && !n.is_nan(),
        Some(_) => true,
    }
}

/// Parses `args` (without the program name) as roblox-ts's `rbxtsc` does.
pub fn parse(args: &[String]) -> Parsed {
    // the top level knows only help and version; its first positional picks the command
    let mut top = Parser::new(TOP_LEVEL_OPTIONS);
    top.parse(args);
    let command = top.positionals.first().is_some_and(|p| p == "build");

    let mut parser = Parser::new(BUILD_OPTIONS);
    parser.parse(args);
    if parser.positionals.last().is_some_and(|p| p == "help") || truthy(parser.get("help")) {
        return Parsed::Help;
    }
    if truthy(parser.get("version")) {
        return Parsed::Version;
    }

    // strict: unknown option keys, then the positionals after the command
    let mut unknown: Vec<String> =
        parser.js_key_order().into_iter().filter(|k| !parser.is_known(k)).map(str::to_owned).collect();
    unknown.extend(parser.positionals.iter().skip(usize::from(command)).cloned());
    if !unknown.is_empty() {
        let list: Vec<String> =
            unknown.iter().map(|s| if s.trim().is_empty() { format!("\"{s}\"") } else { s.clone() }).collect();
        let noun = if list.len() == 1 { "argument" } else { "arguments" };
        return Parsed::Fail(format!("Unknown {noun}: {}", list.join(", ")));
    }

    // choices
    if let Some(value) = parser.get("type") {
        let values = match value {
            Value::Arr(items) => items.clone(),
            other => vec![other.clone()],
        };
        let invalid: Vec<String> = values
            .iter()
            .filter(|v| !matches!(v, Value::Str(s) if TYPE_CHOICES.contains(&s.as_str())))
            .map(Value::to_json)
            .collect();
        if !invalid.is_empty() {
            let choices: Vec<String> = TYPE_CHOICES.iter().map(|c| json_string(c)).collect();
            return Parsed::Fail(format!(
                "Invalid values:\n  Argument: type, Given: {}, Choices: {}",
                invalid.join(", "),
                choices.join(", ")
            ));
        }
    }

    // implications: usePolling -> watch
    if parser.has_key("usePolling") && !parser.has_key("watch") {
        return Parsed::Fail("Missing dependent arguments:\n usePolling -> watch".to_owned());
    }

    let options = BUILD_OPTIONS
        .iter()
        .filter_map(|spec| parser.get(spec.name).map(|v| (spec.name.to_owned(), v.clone())))
        .collect();
    Parsed::Options(options)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helpers() {
        assert_eq!(camel_case("include-path"), "includePath");
        assert_eq!(camel_case("INCLUDE-PATH"), "includePath");
        assert_eq!(camel_case("Include-Path"), "IncludePath");
        assert_eq!(camel_case("-x"), "x");
        assert_eq!(camel_case("no-"), "no");
        assert_eq!(decamelize("includePath"), "include-path");
        assert_eq!(coerce_number("1e3"), Some(1000.0));
        assert_eq!(coerce_number("0x10"), Some(16.0));
        assert_eq!(coerce_number("010"), None);
        assert_eq!(coerce_number("-1"), Some(-1.0));
        assert_eq!(coerce_number("1.50"), Some(1.5));
        assert_eq!(js_number(1.5), "1.5");
        assert_eq!(js_number(-1.0), "-1");
        assert!(is_negative("-1") && is_negative("-.5") && !is_negative("-x") && !is_negative("-"));
    }
}
