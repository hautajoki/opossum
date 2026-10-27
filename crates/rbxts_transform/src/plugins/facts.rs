//! Source facts for plugins: UTF-16 positions with TypeScript's `getStart()` and `computeLineStarts`, source text
//! by span, and JSDoc tags with `{@link}` names resolved.

use rbxts_plugin::{JsDocLink, JsDocTag, Utf16Pos};
use tsrs_ast::positionmap::{PositionMap, compute_position_map};
use tsrs_ast::{Kind, Node, SourceFile};
use tsrs_checker::Checker;
use tsrs_core::P;

/// A file's UTF-8 → UTF-16 map (the line starts are the file's own ECMA line map).
pub(crate) struct FileLines {
    map: PositionMap,
}

impl FileLines {
    pub(crate) fn new(file: P<SourceFile>) -> FileLines {
        FileLines { map: compute_position_map(file.get().text()) }
    }
}

/// `node.getStart()` (trivia skipped, JSDoc not included) and `node.end`, as UTF-8 offsets; `None` for a node with
/// no source position.
pub(crate) fn span(node: P<Node>, file: P<SourceFile>) -> Option<(u32, u32)> {
    let end = node.end().source_offset()?;
    node.pos().source_offset()?;
    let start = tsrs_scanner::get_token_pos_of_node(node, file, false);
    Some((start, end.max(start)))
}

/// The UTF-16 position of a UTF-8 offset: line and character as TypeScript's `getLineAndCharacterOfPosition`
/// counts them (line breaks CR, LF, CRLF, U+2028, U+2029: the file's ECMA line map).
pub(crate) fn utf16_pos_of(file: P<SourceFile>, lines: &FileLines, offset: u32) -> Utf16Pos {
    let line_starts = tsrs_scanner::get_ecma_line_starts(&*file);
    let line = tsrs_scanner::compute_line_of_position(line_starts, offset);
    let line_start = line_starts[line];
    let offset16 = lines.map.utf8_to_utf16(offset);
    Utf16Pos { offset: offset16, line: line as u32, character: offset16 - lines.map.utf8_to_utf16(line_start) }
}

/// The UTF-16 length of a UTF-8 range.
pub(crate) fn utf16_len(lines: &FileLines, start: u32, end: u32) -> u32 {
    lines.map.utf8_to_utf16(end) - lines.map.utf8_to_utf16(start)
}

/// A node's text by its full span, trimmed (for JSDoc names, whose `getStart()` the scanner does not know).
fn name_text(node: P<Node>) -> &'static str {
    let Some(file) = tsrs_ast::get_source_file_of_node(node) else { return "" };
    match (node.pos().source_offset(), node.end().source_offset()) {
        (Some(pos), Some(end)) if pos <= end => file.get().text()[pos as usize..end as usize].trim(),
        _ => "",
    }
}

/// The display text of a `{@link}` (`part`) after its name: the source from the name's end to the closing `}`,
/// trimmed, a leading `|` dropped (`{@link A | text}`), `None` when nothing is left. TypeScript's own `text` is the
/// tokens of that same span (a link ends at its `}` or at the end of the line).
fn link_text(part: P<Node>, name: P<Node>) -> Option<&'static str> {
    let file = tsrs_ast::get_source_file_of_node(part)?;
    let start = name.end().source_offset()? as usize;
    let end = part.end().source_offset()? as usize;
    let text = file.get().text().get(start..end)?;
    let text = text.strip_suffix('}').unwrap_or(text).trim();
    let text = text.strip_prefix('|').map_or(text, str::trim);
    (!text.is_empty()).then_some(text)
}

/// The nodes whose JSDoc belongs to `declaration` (`getJSDocCommentsAndTags`, the common cases): the node itself,
/// and for a variable declaration its variable statement, where TypeScript attaches the comment.
fn jsdoc_hosts(declaration: P<Node>) -> Vec<P<Node>> {
    let mut hosts = vec![declaration];
    if declaration.kind() == Kind::VariableDeclaration
        && let Some(list) = declaration.parent()
        && list.kind() == Kind::VariableDeclarationList
        && let Some(statement) = list.parent()
        && statement.kind() == Kind::VariableStatement
    {
        hosts.push(statement);
    }
    hosts
}

/// The JSDoc tags of a declaration, `{@link}` names resolved on `checker` (aliases not skipped). A link is written
/// into `comment` as its name's text followed by the link's own text (`{@link A.B rest}` → `A.B rest`).
pub(crate) fn jsdoc_tags(checker: &mut Checker, declaration: P<Node>) -> Vec<JsDocTag> {
    let Some(file) = tsrs_ast::get_source_file_of_node(declaration) else { return Vec::new() };
    let mut tags = Vec::new();
    for host in jsdoc_hosts(declaration) {
        for &jsdoc in host.jsdoc(Some(file.get())) {
            let Some(list) = jsdoc.as_jsdoc().tags() else { continue };
            for &tag in list.nodes() {
                let mut comment = String::new();
                let mut links = Vec::new();
                for &part in tag.comments() {
                    match part.kind() {
                        Kind::JSDocText => comment.extend(part.as_jsdoc_text().text().iter().copied()),
                        Kind::JSDocLink | Kind::JSDocLinkCode | Kind::JSDocLinkPlain => {
                            let (name, text) = match part.kind() {
                                Kind::JSDocLink => (part.as_jsdoc_link().name(), part.as_jsdoc_link().text()),
                                Kind::JSDocLinkCode => {
                                    (part.as_jsdoc_link_code().name(), part.as_jsdoc_link_code().text())
                                }
                                _ => (part.as_jsdoc_link_plain().name(), part.as_jsdoc_link_plain().text()),
                            };
                            if let Some(name) = name {
                                comment.push_str(name_text(name));
                                let symbol = checker.get_symbol_at_location_exported(name);
                                links.push(JsDocLink { name, symbol, text: link_text(part, name) });
                            }
                            comment.extend(text.iter().copied());
                        }
                        _ => {}
                    }
                }
                tags.push(JsDocTag {
                    node: tag,
                    name: name_text(tag.tag_name()),
                    comment: comment.trim().to_owned(),
                    links,
                });
            }
        }
    }
    tags
}
