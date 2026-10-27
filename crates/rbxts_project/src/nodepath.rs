//! Node's `path` module, as the JavaScript packages we port use it: `win32` on Windows, `posix` elsewhere.
//!
//! Output paths and Rojo lookups are compared as strings in roblox-ts (`Map` keys, `startsWith`), so the exact
//! spelling Node produces (separators, drive-letter case, trailing separators) matters, not just the file it names.
//! Both flavours are always compiled so that tests can check either on any host.

#[cfg(windows)]
pub use win32::*;

#[cfg(not(windows))]
pub use posix::*;

/// Where `to` lies from `from`, as the sign of `relative(from, to)` tells it (both resolved first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relation {
    /// The same path (`relative` is empty).
    Same,
    /// Below `from`.
    Inside {
        /// The first step is a name starting with `..` (such as `..x`), which a `startsWith("..")` test mistakes for
        /// "outside".
        dotdot_name: bool,
    },
    /// Elsewhere on the same root (`relative` starts with a `..` step).
    Outside,
    /// On another root (`relative` is absolute: `to` itself).
    OtherRoot,
}

/// The `Relation` of a `relative` result.
fn relation_of(relative: &str, sep: char, is_absolute: fn(&str) -> bool) -> Relation {
    if relative.is_empty() {
        Relation::Same
    } else if is_absolute(relative) {
        Relation::OtherRoot
    } else if relative == ".." || (relative.starts_with("..") && relative[2..].starts_with(sep)) {
        Relation::Outside
    } else {
        Relation::Inside { dotdot_name: relative.starts_with("..") }
    }
}

/// The process's current directory, as Node's `process.cwd()` spells it.
fn process_cwd(fallback: &str) -> String {
    std::env::current_dir().map(|d| d.to_string_lossy().into_owned()).unwrap_or_else(|_| fallback.to_owned())
}

/// The segment loop shared by both flavours (Node's `normalizeString`): drops empty and `.` segments and resolves
/// `..` against the previous segment. A `..` with nothing to remove is kept only when `allow_above_root`.
fn normalize_segments(path: &str, allow_above_root: bool, is_sep: fn(char) -> bool, sep: char) -> String {
    let mut segments: Vec<&str> = Vec::new();
    for segment in path.split(is_sep) {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.last().is_some_and(|&last| last != "..") {
                    segments.pop();
                } else if allow_above_root {
                    segments.push("..");
                }
            }
            _ => segments.push(segment),
        }
    }
    let mut out = String::with_capacity(path.len());
    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            out.push(sep);
        }
        out.push_str(segment);
    }
    out
}

/// Node's `extname` scan over the last segment of `path` (`start` skips a drive prefix).
fn extname_impl(path: &str, start: usize, is_sep: fn(char) -> bool) -> &str {
    let chars: Vec<(usize, char)> = path.char_indices().filter(|&(i, _)| i >= start).collect();
    let mut start_dot: Option<usize> = None; // index into `chars`
    let mut start_part = 0usize;
    let mut end: Option<usize> = None;
    let mut matched_slash = true;
    // 0: no dot yet or only dots before it; 1: another dot right before; -1: a non-dot before the last dot.
    let mut pre_dot_state = 0i32;
    for k in (0..chars.len()).rev() {
        let c = chars[k].1;
        if is_sep(c) {
            if !matched_slash {
                start_part = k + 1;
                break;
            }
            continue;
        }
        if end.is_none() {
            matched_slash = false;
            end = Some(k + 1);
        }
        if c == '.' {
            if start_dot.is_none() {
                start_dot = Some(k);
            } else if pre_dot_state != 1 {
                pre_dot_state = 1;
            }
        } else if start_dot.is_some() {
            pre_dot_state = -1;
        }
    }
    let (Some(start_dot), Some(end)) = (start_dot, end) else { return "" };
    if pre_dot_state == 0 || (pre_dot_state == 1 && start_dot == end - 1 && start_dot == start_part + 1) {
        return "";
    }
    let byte = |k: usize| chars.get(k).map_or(path.len(), |&(i, _)| i);
    &path[byte(start_dot)..byte(end)]
}

/// Node's `basename(path, suffix)` scan (`start` skips a drive prefix): the suffix is matched from the end of the
/// last segment and removed when it is a proper suffix of it. Returns None without a usable suffix (plain basename).
fn basename_suffix_impl<'a>(path: &'a str, start: usize, suffix: &str, is_sep: fn(char) -> bool) -> Option<&'a str> {
    if suffix.is_empty() || suffix.len() > path.len() {
        return None;
    }
    if suffix == path {
        return Some("");
    }
    let b = path.as_bytes();
    let s = suffix.as_bytes();
    let mut start = start;
    let mut end: Option<usize> = None;
    let mut matched_slash = true;
    let mut ext_idx = s.len() as isize - 1;
    let mut first_non_slash_end: Option<usize> = None;
    let mut i = b.len();
    while i > start {
        i -= 1;
        let code = b[i];
        if is_sep(code as char) {
            if !matched_slash {
                start = i + 1;
                break;
            }
        } else {
            if first_non_slash_end.is_none() {
                matched_slash = false;
                first_non_slash_end = Some(i + 1);
            }
            if ext_idx >= 0 {
                if code == s[ext_idx as usize] {
                    ext_idx -= 1;
                    if ext_idx == -1 {
                        end = Some(i);
                    }
                } else {
                    ext_idx = -1;
                    end = first_non_slash_end;
                }
            }
        }
    }
    let end = match end {
        Some(e) if e == start => first_non_slash_end.unwrap_or(start),
        Some(e) => e,
        None => b.len(),
    };
    // Byte-wise matching can stop inside a multi-byte character only when the suffix itself does; fall back then.
    Some(path.get(start..end.max(start)).unwrap_or(""))
}

/// `path.win32`.
pub mod win32 {
    /// `path.sep`.
    pub const SEP: char = '\\';

    /// Both separators are accepted on input.
    pub fn is_sep(c: char) -> bool {
        c == '\\' || c == '/'
    }

    fn is_drive_letter(c: char) -> bool {
        c.is_ascii_alphabetic()
    }

    /// Node's root match of a path (bytes).
    #[derive(Default)]
    struct Root {
        /// As Node spells it: `C:`, `\\server\share` (one backslash between the parts, however many separators the
        /// path has), or a device root `\\.` / `\\?`; empty without one.
        device: String,
        is_absolute: bool,
        root_end: usize,
        /// For a `\\server\share` match (a device root's too): where the share part ends. `dirname` keeps it.
        unc_end: Option<usize>,
        /// `\\.\…` or `\\?\…`: the device is `\\.` or `\\?` and the root ends after its separator.
        device_root: bool,
    }

    fn root(path: &str) -> Root {
        let b = path.as_bytes();
        let len = b.len();
        if len == 0 {
            return Root::default();
        }
        let c0 = b[0] as char;
        if len == 1 {
            return if is_sep(c0) { Root { is_absolute: true, root_end: 1, ..Root::default() } } else { Root::default() };
        }
        if is_sep(c0) {
            if is_sep(b[1] as char) {
                // UNC: \\server\share (separators between the parts collapse), or a device root \\.\ or \\?\
                let mut j = 2;
                let last = j;
                while j < len && !is_sep(b[j] as char) {
                    j += 1;
                }
                if j < len && j != last {
                    let first_part = &path[last..j];
                    let mut last = j;
                    while j < len && is_sep(b[j] as char) {
                        j += 1;
                    }
                    if j < len && j != last {
                        last = j;
                        while j < len && !is_sep(b[j] as char) {
                            j += 1;
                        }
                        if first_part == "." || first_part == "?" {
                            return Root {
                                device: format!("\\\\{first_part}"),
                                is_absolute: true,
                                root_end: 4,
                                unc_end: Some(j),
                                device_root: true,
                            };
                        }
                        return Root {
                            device: format!("\\\\{first_part}\\{}", &path[last..j]),
                            is_absolute: true,
                            root_end: j,
                            unc_end: Some(j),
                            device_root: false,
                        };
                    }
                }
            }
            return Root { is_absolute: true, root_end: 1, ..Root::default() };
        }
        if is_drive_letter(c0) && b[1] == b':' {
            let is_absolute = len > 2 && is_sep(b[2] as char);
            return Root {
                device: path[..2].to_owned(),
                is_absolute,
                root_end: if is_absolute { 3 } else { 2 },
                ..Root::default()
            };
        }
        Root::default()
    }

    /// Already what `resolve` would return: `C:\` and single backslashes, no `.` or `..` segments, no trailing
    /// separator.
    fn is_resolved_form(p: &str) -> bool {
        let b = p.as_bytes();
        if b.len() < 3 || !is_drive_letter(b[0] as char) || b[1] != b':' || b[2] != b'\\' {
            return false;
        }
        if b.len() == 3 {
            return true;
        }
        if b[b.len() - 1] == b'\\' || b.contains(&b'/') {
            return false;
        }
        p[3..].split('\\').all(|seg| !seg.is_empty() && seg != "." && seg != "..")
    }

    /// `path.win32.normalize`.
    pub fn normalize(path: &str) -> String {
        let len = path.len();
        if len == 0 {
            return ".".into();
        }
        if len == 1 {
            return if path == "/" { "\\".into() } else { path.into() };
        }
        let b = path.as_bytes();
        let Root { device, is_absolute, root_end, unc_end, device_root } = root(path);
        // A UNC path that is only `\\server\share` gets a trailing separator.
        if unc_end == Some(len) && !device_root {
            return format!("{device}\\");
        }
        let mut tail = if root_end < len {
            super::normalize_segments(&path[root_end..], !is_absolute, is_sep, SEP)
        } else {
            String::new()
        };
        if tail.is_empty() && !is_absolute {
            tail.push('.');
        }
        if !tail.is_empty() && is_sep(b[len - 1] as char) {
            tail.push(SEP);
        }
        // Node 23+ (nodejs/node#56123): a relative result must not read as a drive path.
        if !is_absolute && device.is_empty() && path.contains(':') {
            let t = tail.as_bytes();
            let drive_like = t.len() >= 2 && is_drive_letter(t[0] as char) && t[1] == b':';
            let colon_ends_segment = path.match_indices(':').any(|(i, _)| i == len - 1 || is_sep(b[i + 1] as char));
            if drive_like || colon_ends_segment {
                return format!(".\\{tail}");
            }
        }
        if device.is_empty() {
            if is_absolute { format!("\\{tail}") } else { tail }
        } else if is_absolute {
            format!("{device}\\{tail}")
        } else {
            format!("{device}{tail}")
        }
    }

    /// `path.win32.join`.
    pub fn join(parts: &[&str]) -> String {
        let mut joined = String::new();
        let mut first: Option<&str> = None;
        for part in parts.iter().filter(|p| !p.is_empty()) {
            if first.is_none() {
                first = Some(part);
            } else {
                joined.push(SEP);
            }
            joined.push_str(part);
        }
        let Some(first) = first else { return ".".into() };
        // Keep the joined path from starting with two separators unless the first part already did (a UNC root).
        let fb = first.as_bytes();
        let mut needs_replace = true;
        let mut slash_count = 0;
        if is_sep(fb[0] as char) {
            slash_count += 1;
            if fb.len() > 1 && is_sep(fb[1] as char) {
                slash_count += 1;
                if fb.len() > 2 {
                    if is_sep(fb[2] as char) {
                        slash_count += 1;
                    } else {
                        needs_replace = false;
                    }
                }
            }
        }
        if needs_replace {
            let jb = joined.as_bytes();
            while slash_count < jb.len() && is_sep(jb[slash_count] as char) {
                slash_count += 1;
            }
            if slash_count >= 2 {
                joined = format!("\\{}", &joined[slash_count..]);
            }
        }
        normalize(&joined)
    }

    /// `path.win32.resolve`, against the process's current directory.
    pub fn resolve(parts: &[&str]) -> String {
        resolve_with(&|| super::process_cwd("\\"), parts)
    }

    /// `path.win32.resolve` with an explicit current directory (also used for another drive's current directory,
    /// which Node reads from the `=C:` variables; this falls back to that drive's root as Node does without one).
    pub fn resolve_from(cwd: &str, parts: &[&str]) -> String {
        resolve_with(&|| cwd.to_owned(), parts)
    }

    /// The current directory is read only when the parts do not make an absolute path on their own.
    fn resolve_with(cwd: &dyn Fn() -> String, parts: &[&str]) -> String {
        if let [only] = parts
            && is_resolved_form(only)
        {
            return (*only).to_owned();
        }
        let mut resolved_device = String::new();
        let mut resolved_tail = String::new();
        let mut resolved_absolute = false;
        let mut i = parts.len() as isize - 1;
        loop {
            if i < -1 {
                break;
            }
            let owned;
            let path: &str = if i >= 0 {
                let p = parts[i as usize];
                if p.is_empty() {
                    i -= 1;
                    continue;
                }
                p
            } else if resolved_device.is_empty() {
                owned = cwd();
                &owned
            } else {
                // Node: the current directory unless its first two characters name another device and a
                // backslash follows them
                let cwd = cwd();
                let head: String = cwd.chars().take(2).collect();
                owned = if head.to_lowercase() != resolved_device.to_lowercase() && cwd.as_bytes().get(2) == Some(&b'\\')
                {
                    format!("{resolved_device}\\")
                } else {
                    cwd
                };
                &owned
            };
            let Root { device, is_absolute, root_end, .. } = root(path);
            if !device.is_empty() {
                if !resolved_device.is_empty() {
                    if device.to_lowercase() != resolved_device.to_lowercase() {
                        i -= 1;
                        continue;
                    }
                } else {
                    resolved_device = device;
                }
            }
            if resolved_absolute {
                if !resolved_device.is_empty() {
                    break;
                }
            } else {
                resolved_tail = format!("{}\\{}", &path[root_end..], resolved_tail);
                resolved_absolute = is_absolute;
                if is_absolute && !resolved_device.is_empty() {
                    break;
                }
            }
            i -= 1;
        }
        let tail = super::normalize_segments(&resolved_tail, !resolved_absolute, is_sep, SEP);
        if resolved_absolute {
            format!("{resolved_device}\\{tail}")
        } else {
            let out = format!("{resolved_device}{tail}");
            if out.is_empty() { ".".into() } else { out }
        }
    }

    /// `path.win32.relative`: compares case-insensitively and keeps `to`'s spelling. Paths on different roots give
    /// the resolved `to`.
    pub fn relative(from: &str, to: &str) -> String {
        relative_with(&|| super::process_cwd(""), from, to)
    }

    /// `relative` with an explicit current directory.
    pub fn relative_from(cwd: &str, from: &str, to: &str) -> String {
        relative_with(&|| cwd.to_owned(), from, to)
    }

    fn relative_with(cwd: &dyn Fn() -> String, from: &str, to: &str) -> String {
        if from == to {
            return String::new();
        }
        let from_orig = resolve_with(cwd, &[from]);
        let to_orig = resolve_with(cwd, &[to]);
        if from_orig == to_orig {
            return String::new();
        }
        // Node lowercases with `toLowerCase` and indexes UTF-16 units; for ASCII paths bytes are the same thing
        if from_orig.is_ascii() && to_orig.is_ascii() {
            let from_l = from_orig.to_ascii_lowercase();
            let to_l = to_orig.to_ascii_lowercase();
            return relative_core(from_l.as_bytes(), to_l.as_bytes(), b'\\', &to_orig, |a, b| to_orig[a..b].to_owned());
        }
        // `toLowerCase` is Unicode's full lowercase mapping (`İ` becomes two units), as Rust's `to_lowercase`
        let from_l = from_orig.to_lowercase();
        let to_l = to_orig.to_lowercase();
        if from_l == to_l {
            return String::new();
        }
        let units = |s: &str| -> Vec<u16> { s.encode_utf16().collect() };
        let (from_u, to_u, from_lu, to_lu) = (units(&from_orig), units(&to_orig), units(&from_l), units(&to_l));
        if from_u.len() != from_lu.len() || to_u.len() != to_lu.len() {
            return relative_by_segments(&from_orig, &to_orig);
        }
        let slice = |a: usize, b: usize| String::from_utf16_lossy(&to_u[a.min(b).min(to_u.len())..b.min(to_u.len())]);
        relative_core(&from_lu, &to_lu, u16::from(b'\\'), &to_orig, slice)
    }

    /// Node 23's `win32.relative` when lowercasing changes a path's length in UTF-16 units: segment by segment.
    fn relative_by_segments(from_orig: &str, to_orig: &str) -> String {
        let mut from: Vec<&str> = from_orig.split('\\').collect();
        let mut to: Vec<&str> = to_orig.split('\\').collect();
        if from.last() == Some(&"") {
            from.pop();
        }
        if to.last() == Some(&"") {
            to.pop();
        }
        let length = from.len().min(to.len());
        let i = (0..length).find(|&i| from[i].to_lowercase() != to[i].to_lowercase()).unwrap_or(length);
        if i == 0 {
            return to_orig.to_owned();
        }
        if i == length {
            if to.len() > length {
                return to[i..].join("\\");
            }
            if from.len() > length {
                return "..\\".repeat(from.len() - 1 - i) + "..";
            }
            return String::new();
        }
        "..\\".repeat(from.len() - i) + &to[i..].join("\\")
    }

    /// Node's `win32.relative` after both paths are resolved, over lowercased units; `slice` cuts `to`'s original
    /// spelling by unit index.
    fn relative_core<T: Copy + PartialEq>(
        from_l: &[T],
        to_l: &[T],
        bs: T,
        to_orig: &str,
        slice: impl Fn(usize, usize) -> String,
    ) -> String {
        if from_l == to_l {
            return String::new();
        }
        let to_units = to_l.len();
        let mut from_start = 0;
        while from_start < from_l.len() && from_l[from_start] == bs {
            from_start += 1;
        }
        let mut from_end = from_l.len();
        while from_end > from_start + 1 && from_l[from_end - 1] == bs {
            from_end -= 1;
        }
        let from_len = from_end - from_start;
        let mut to_start = 0;
        while to_start < to_l.len() && to_l[to_start] == bs {
            to_start += 1;
        }
        let mut to_end = to_l.len();
        while to_end > to_start + 1 && to_l[to_end - 1] == bs {
            to_end -= 1;
        }
        let to_len = to_end - to_start;
        let length = from_len.min(to_len);
        let mut last_common_sep: isize = -1;
        let mut i = 0;
        while i < length {
            let c = from_l[from_start + i];
            if c != to_l[to_start + i] {
                break;
            } else if c == bs {
                last_common_sep = i as isize;
            }
            i += 1;
        }
        if i != length {
            if last_common_sep == -1 {
                return to_orig.to_owned();
            }
        } else {
            if to_len > length {
                if to_l[to_start + i] == bs {
                    return slice(to_start + i + 1, to_units);
                }
                if i == 2 {
                    return slice(to_start + i, to_units);
                }
            }
            if from_len > length {
                if from_l[from_start + i] == bs {
                    last_common_sep = i as isize;
                } else if i == 2 {
                    last_common_sep = 3;
                }
            }
            if last_common_sep == -1 {
                last_common_sep = 0;
            }
        }
        let mut out = String::new();
        let mut k = (from_start as isize + last_common_sep + 1) as usize;
        while k <= from_end {
            if k == from_end || from_l[k] == bs {
                out.push_str(if out.is_empty() { ".." } else { "\\.." });
            }
            k += 1;
        }
        let mut to_start = (to_start as isize + last_common_sep) as usize;
        if !out.is_empty() {
            out.push_str(&slice(to_start, to_end));
            return out;
        }
        if to_start < to_units && to_l[to_start] == bs {
            to_start += 1;
        }
        slice(to_start, to_end)
    }

    /// `Relation` of `to` from `from` (`relative` without building it when both are already resolved ASCII paths,
    /// as partition checks call it for every file and partition).
    pub fn relation(from: &str, to: &str) -> super::Relation {
        use super::Relation;
        if from.is_ascii() && to.is_ascii() && is_resolved_form(from) && is_resolved_form(to) {
            let (f, t) = (from.as_bytes(), to.as_bytes());
            if !f[0].eq_ignore_ascii_case(&t[0]) {
                return Relation::OtherRoot;
            }
            if f.eq_ignore_ascii_case(t) {
                return Relation::Same;
            }
            // `C:\\` already ends with its separator
            let prefix = if f.len() == 3 { 3 } else { f.len() + 1 };
            let inside =
                t.len() > prefix && t[..f.len()].eq_ignore_ascii_case(f) && (f.len() == 3 || t[f.len()] == b'\\');
            return if inside {
                Relation::Inside { dotdot_name: t[prefix..].starts_with(b"..") }
            } else {
                Relation::Outside
            };
        }
        super::relation_of(&relative(from, to), SEP, is_absolute)
    }

    /// `path.win32.dirname`.
    pub fn dirname(path: &str) -> &str {
        let b = path.as_bytes();
        let len = b.len();
        if len == 0 {
            return ".";
        }
        if len == 1 {
            return if is_sep(b[0] as char) { path } else { "." };
        }
        let mut root_end: Option<usize> = None;
        let mut offset = 0;
        if is_sep(b[0] as char) {
            root_end = Some(1);
            offset = 1;
            if is_sep(b[1] as char)
                && let Some(end) = root(path).unc_end
            {
                if end == len {
                    // The whole path is a UNC root.
                    return path;
                }
                root_end = Some(end + 1);
                offset = end + 1;
            }
        } else if is_drive_letter(b[0] as char) && b[1] == b':' {
            let end = if len > 2 && is_sep(b[2] as char) { 3 } else { 2 };
            root_end = Some(end);
            offset = end;
        }
        let mut end: Option<usize> = None;
        let mut matched_slash = true;
        let mut i = len;
        while i > offset {
            i -= 1;
            if is_sep(b[i] as char) {
                if !matched_slash {
                    end = Some(i);
                    break;
                }
            } else {
                matched_slash = false;
            }
        }
        match (end, root_end) {
            (Some(end), _) => &path[..end],
            (None, Some(root_end)) => &path[..root_end],
            (None, None) => ".",
        }
    }

    /// `path.win32.basename(path)`.
    pub fn basename(path: &str) -> &str {
        let b = path.as_bytes();
        let mut start = 0;
        if b.len() >= 2 && is_drive_letter(b[0] as char) && b[1] == b':' {
            start = 2;
        }
        let mut end: Option<usize> = None;
        let mut i = b.len();
        while i > start {
            i -= 1;
            if is_sep(b[i] as char) {
                if end.is_some() {
                    start = i + 1;
                    break;
                }
            } else if end.is_none() {
                end = Some(i + 1);
            }
        }
        match end {
            Some(end) => &path[start..end],
            None => "",
        }
    }

    /// `path.win32.basename(path, suffix)`.
    pub fn basename_without<'a>(path: &'a str, suffix: &str) -> &'a str {
        let b = path.as_bytes();
        let start = if b.len() >= 2 && is_drive_letter(b[0] as char) && b[1] == b':' { 2 } else { 0 };
        super::basename_suffix_impl(path, start, suffix, is_sep).unwrap_or_else(|| basename(path))
    }

    /// `path.win32.extname`.
    pub fn extname(path: &str) -> &str {
        let b = path.as_bytes();
        let start = if b.len() >= 2 && b[1] == b':' && is_drive_letter(b[0] as char) { 2 } else { 0 };
        super::extname_impl(path, start, is_sep)
    }

    /// `path.win32.isAbsolute`.
    pub fn is_absolute(path: &str) -> bool {
        let b = path.as_bytes();
        match b.len() {
            0 => false,
            _ if is_sep(b[0] as char) => true,
            n => n > 2 && is_drive_letter(b[0] as char) && b[1] == b':' && is_sep(b[2] as char),
        }
    }
}

/// `path.posix`.
pub mod posix {
    /// `path.sep`.
    pub const SEP: char = '/';

    /// Only `/` separates.
    pub fn is_sep(c: char) -> bool {
        c == '/'
    }

    /// `path.posix.normalize`.
    pub fn normalize(path: &str) -> String {
        if path.is_empty() {
            return ".".into();
        }
        let is_absolute = path.starts_with('/');
        let trailing = path.ends_with('/');
        let mut out = super::normalize_segments(path, !is_absolute, is_sep, SEP);
        if out.is_empty() {
            if is_absolute {
                return "/".into();
            }
            return if trailing { "./".into() } else { ".".into() };
        }
        if trailing {
            out.push('/');
        }
        if is_absolute { format!("/{out}") } else { out }
    }

    /// `path.posix.join`.
    pub fn join(parts: &[&str]) -> String {
        let joined: Vec<&str> = parts.iter().copied().filter(|p| !p.is_empty()).collect();
        if joined.is_empty() {
            return ".".into();
        }
        normalize(&joined.join("/"))
    }

    /// `path.posix.resolve`, against the process's current directory.
    pub fn resolve(parts: &[&str]) -> String {
        if parts.iter().any(|p| p.starts_with('/')) {
            return resolve_from("/", parts);
        }
        resolve_from(&super::process_cwd("/"), parts)
    }

    /// `path.posix.resolve` with an explicit current directory.
    pub fn resolve_from(cwd: &str, parts: &[&str]) -> String {
        let mut resolved = String::new();
        let mut absolute = false;
        for p in parts.iter().rev().copied().chain(std::iter::once(cwd)) {
            if p.is_empty() {
                continue;
            }
            resolved = format!("{p}/{resolved}");
            absolute = p.starts_with('/');
            if absolute {
                break;
            }
        }
        let tail = super::normalize_segments(&resolved, !absolute, is_sep, SEP);
        if absolute {
            format!("/{tail}")
        } else if tail.is_empty() {
            ".".into()
        } else {
            tail
        }
    }

    /// `path.posix.relative`.
    pub fn relative(from: &str, to: &str) -> String {
        if from.starts_with('/') && to.starts_with('/') {
            return relative_from("/", from, to);
        }
        relative_from(&super::process_cwd("/"), from, to)
    }

    /// `relative` with an explicit current directory.
    pub fn relative_from(cwd: &str, from: &str, to: &str) -> String {
        if from == to {
            return String::new();
        }
        let from = resolve_from(cwd, &[from]);
        let to = resolve_from(cwd, &[to]);
        if from == to {
            return String::new();
        }
        let f = from.as_bytes();
        let t = to.as_bytes();
        let from_start = 1;
        let from_end = f.len();
        let from_len = from_end - from_start;
        let to_start = 1;
        let to_len = t.len() - to_start;
        let length = from_len.min(to_len);
        let mut last_common_sep: isize = -1;
        let mut i = 0;
        while i < length {
            let c = f[from_start + i];
            if c != t[to_start + i] {
                break;
            } else if c == b'/' {
                last_common_sep = i as isize;
            }
            i += 1;
        }
        if i == length {
            if to_len > length {
                if t[to_start + i] == b'/' {
                    return to[to_start + i + 1..].to_string();
                }
                if i == 0 {
                    return to[to_start + i..].to_string();
                }
            } else if from_len > length {
                if f[from_start + i] == b'/' {
                    last_common_sep = i as isize;
                } else if i == 0 {
                    last_common_sep = 0;
                }
            }
        }
        let mut out = String::new();
        let mut k = (from_start as isize + last_common_sep + 1) as usize;
        while k <= from_end {
            if k == from_end || f[k] == b'/' {
                out.push_str(if out.is_empty() { ".." } else { "/.." });
            }
            k += 1;
        }
        format!("{out}{}", &to[(to_start as isize + last_common_sep) as usize..])
    }

    /// `Relation` of `to` from `from`.
    pub fn relation(from: &str, to: &str) -> super::Relation {
        super::relation_of(&relative(from, to), SEP, is_absolute)
    }

    /// `path.posix.dirname`.
    pub fn dirname(path: &str) -> &str {
        if path.is_empty() {
            return ".";
        }
        let b = path.as_bytes();
        let has_root = b[0] == b'/';
        let mut end: Option<usize> = None;
        let mut matched_slash = true;
        let mut i = b.len();
        while i > 1 {
            i -= 1;
            if b[i] == b'/' {
                if !matched_slash {
                    end = Some(i);
                    break;
                }
            } else {
                matched_slash = false;
            }
        }
        match end {
            None => {
                if has_root {
                    "/"
                } else {
                    "."
                }
            }
            Some(1) if has_root => "//",
            Some(end) => &path[..end],
        }
    }

    /// `path.posix.basename(path)`.
    pub fn basename(path: &str) -> &str {
        let trimmed = path.trim_end_matches('/');
        match trimmed.rfind('/') {
            Some(i) => &trimmed[i + 1..],
            None => trimmed,
        }
    }

    /// `path.posix.basename(path, suffix)`.
    pub fn basename_without<'a>(path: &'a str, suffix: &str) -> &'a str {
        super::basename_suffix_impl(path, 0, suffix, is_sep).unwrap_or_else(|| basename(path))
    }

    /// `path.posix.extname`.
    pub fn extname(path: &str) -> &str {
        super::extname_impl(path, 0, is_sep)
    }

    /// `path.posix.isAbsolute`.
    pub fn is_absolute(path: &str) -> bool {
        path.starts_with('/')
    }
}

#[cfg(test)]
mod tests {
    use super::{posix, win32};

    #[test]
    fn win32_normalize_and_join() {
        assert_eq!(win32::normalize("E:/a/b/../c/./d/"), "E:\\a\\c\\d\\");
        assert_eq!(win32::normalize("e:/a//b"), "e:\\a\\b");
        assert_eq!(win32::normalize("a/../.."), "..");
        assert_eq!(win32::normalize(""), ".");
        assert_eq!(win32::normalize("C:"), "C:.");
        assert_eq!(win32::normalize("//server/share"), "\\\\server\\share\\");
        assert_eq!(win32::join(&["E:\\a\\", "..", "\\"]), "E:\\");
        assert_eq!(win32::join(&["E:/out", "x/init.luau"]), "E:\\out\\x\\init.luau");
        assert_eq!(win32::join(&["", ""]), ".");
    }

    #[test]
    fn win32_resolve_relative() {
        let cwd = "E:\\work\\proj";
        assert_eq!(win32::resolve_from(cwd, &["src"]), "E:\\work\\proj\\src");
        assert_eq!(win32::resolve_from(cwd, &["E:/a/b/", "../c"]), "E:\\a\\c");
        assert_eq!(win32::resolve_from(cwd, &["\\x"]), "E:\\x");
        assert_eq!(win32::resolve_from(cwd, &["D:x"]), "D:\\x");
        assert_eq!(win32::relative_from(cwd, "E:\\a\\b", "E:\\a\\b\\c\\d"), "c\\d");
        assert_eq!(win32::relative_from(cwd, "E:\\A\\b", "e:\\a\\B\\C"), "C");
        assert_eq!(win32::relative_from(cwd, "E:\\a\\b\\c", "E:\\a\\x"), "..\\..\\x");
        assert_eq!(win32::relative_from(cwd, "E:\\a\\b", "E:\\a\\b"), "");
        assert_eq!(win32::relative_from(cwd, "E:\\a\\b", "E:\\A\\B"), "");
        assert_eq!(win32::relative_from(cwd, "E:\\a", "D:\\a"), "D:\\a");
        assert_eq!(win32::relative_from(cwd, "E:\\", "E:\\foo"), "foo");
        assert_eq!(win32::relative_from(cwd, "E:\\foo", "E:\\"), "..");
        assert_eq!(win32::relative_from(cwd, "E:\\src", "E:\\src-extra\\a"), "..\\src-extra\\a");
    }

    #[test]
    fn win32_parts() {
        assert_eq!(win32::dirname("E:/a/b/c.ts"), "E:/a/b");
        assert_eq!(win32::dirname("E:\\a"), "E:\\");
        assert_eq!(win32::dirname("E:\\"), "E:\\");
        assert_eq!(win32::dirname("a"), ".");
        assert_eq!(win32::basename("E:\\a\\b.ts\\"), "b.ts");
        assert_eq!(win32::basename("C:"), "");
        assert_eq!(win32::basename_without("x/a.server.luau", ".luau"), "a.server");
        assert_eq!(win32::basename_without(".luau", ".luau"), "");
        assert_eq!(win32::extname("a/b.server.luau"), ".luau");
        assert_eq!(win32::extname(".index"), "");
        assert_eq!(win32::extname(".index.md"), ".md");
        assert_eq!(win32::extname("index."), ".");
        assert_eq!(win32::extname(".."), "");
        assert_eq!(win32::extname("..a"), ".a");
        assert_eq!(win32::extname("a.b\\c"), "");
        assert!(win32::is_absolute("E:/x") && win32::is_absolute("\\x") && !win32::is_absolute("E:x"));
    }

    #[test]
    fn posix_basics() {
        assert_eq!(posix::normalize("/a/b/../c/"), "/a/c/");
        assert_eq!(posix::join(&["/a/", "..", "/"]), "/");
        assert_eq!(posix::resolve_from("/w", &["x", "../y"]), "/w/y");
        assert_eq!(posix::relative_from("/w", "/a/b", "/a/c/d"), "../c/d");
        assert_eq!(posix::relative_from("/w", "/", "/a"), "a");
        assert_eq!(posix::relative_from("/w", "/a", "/"), "..");
        assert_eq!(posix::dirname("/a/b/"), "/a");
        assert_eq!(posix::dirname("/a"), "/");
        assert_eq!(posix::basename("/a/b.ts/"), "b.ts");
        assert_eq!(posix::extname("/a/.b"), "");
    }
}
