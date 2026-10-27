//! `util/cleanModuleName.ts`

/// `name.replace(/\W/g, "_")`: one `_` per UTF-16 code unit of each non-word character, as JavaScript does.
pub fn clean_module_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' {
            out.push(c);
        } else {
            out.extend(std::iter::repeat_n('_', c.len_utf16()));
        }
    }
    out
}
