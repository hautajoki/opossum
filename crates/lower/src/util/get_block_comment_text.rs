//! `util/getBlockCommentText.ts`: a block comment as Luau comment text, which renders several lines in an indented
//! `--[[ ]]` block, so the source layout of the delimiters, gutter and indentation is removed.

/// JavaScript's `trimStart` / `trimEnd` whitespace (`String.prototype.trim`).
fn is_js_space(c: char) -> bool {
    rbxts_luau::is_js_whitespace(c)
}

/// `/^[ \t]*\*/`
fn has_gutter(line: &str) -> bool {
    line.trim_start_matches([' ', '\t']).starts_with('*')
}

/// `line.replace(/^[ \t]*\* ?/, "")`
fn strip_gutter(line: &str) -> String {
    let rest = line.trim_start_matches([' ', '\t']);
    match rest.strip_prefix('*') {
        Some(rest) => rest.strip_prefix(' ').unwrap_or(rest).to_owned(),
        None => line.to_owned(),
    }
}

/// `getBlockCommentText`
pub fn get_block_comment_text(source: &str) -> String {
    // the second * of a JSDoc comment's /** delimiter is not part of its text
    let start = if source.starts_with("/**") { 3 } else { 2 };
    let text = &source[start.min(source.len())..source.len().saturating_sub(2).max(start.min(source.len()))];

    // match TypeScript's normalization of physical line endings
    let text = text.replace("\r\n", "\n").replace('\r', "\n");

    // trailing whitespace never matters in a comment, and removing it keeps blank lines empty
    let lines: Vec<&str> = text.split('\n').map(|line| line.trim_end_matches(is_js_space)).collect();

    // text on the first line follows the opening delimiter, so only later lines have source indentation
    let first = lines[0].trim_start_matches(is_js_space).to_owned();
    let mut rest: Vec<String> = lines[1..].iter().map(|l| (*l).to_owned()).collect();

    // remove a gutter that every line has, e.g. " * " in a JSDoc comment
    if rest.iter().all(|line| line.is_empty() || has_gutter(line)) {
        rest = rest.iter().map(|line| strip_gutter(line)).collect();
    }

    // remove the indentation shared by later lines, since the renderer indents them inside --[[ ]]
    // (lengths in UTF-16 code units, as JavaScript measures them)
    let utf16_len = |s: &str| s.encode_utf16().count();
    let mut indent = usize::MAX;
    for line in &rest {
        if !line.is_empty() {
            indent = indent.min(utf16_len(line) - utf16_len(line.trim_start_matches(is_js_space)));
        }
    }
    let rest: Vec<String> = rest
        .iter()
        .map(|line| {
            // `line.slice(indent)` with Infinity slices to ""
            let units: Vec<u16> = line.encode_utf16().collect();
            if indent >= units.len() { String::new() } else { String::from_utf16_lossy(&units[indent..]) }
        })
        .collect();

    // drop the lines left empty by delimiters on their own lines, e.g. /** and */
    let mut result = vec![first];
    result.extend(rest);
    if result.first().is_some_and(String::is_empty) {
        result.remove(0);
    }
    if result.last().is_some_and(String::is_empty) {
        result.pop();
    }

    let text = result.join("\n");

    // one line renders as --text, so keep a space after the dashes to avoid starting --[[ or a --! directive
    if result.len() == 1 && !text.is_empty() {
        return format!(" {text}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::get_block_comment_text;

    #[test]
    fn block_comments() {
        assert_eq!(get_block_comment_text("/* hi */"), " hi");
        assert_eq!(get_block_comment_text("/**\n * a\n * b\n */"), "a\nb");
        assert_eq!(get_block_comment_text("/**/"), "");
        assert_eq!(get_block_comment_text("/* a\n     b\n       c */"), "a\nb\n  c");
    }
}
