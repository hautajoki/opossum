//! `Project/transformers/builtin/transformPaths.ts` (a modified `typescript-transform-paths`): in the declarations
//! roblox-ts writes, every module specifier that resolves (with the program's options, `baseUrl` and `paths`
//! included) to a file outside `node_modules` becomes the relative path to that file, without an implicit extension.
//! It does nothing unless the options set `baseUrl` or `paths`.
//!
//! rbxts-rs: TypeScript runs it on the declaration tree before printing; tsrs's emitter takes no transformers, so it
//! runs on text instead: the declaration text is parsed, and each rewritten specifier's string literal token is
//! replaced by the literal TypeScript's printer prints for the new, synthesized one (double quotes, escaped, non-ASCII
//! characters as `\uXXXX`). The printed text is the same, as nothing else in the tree changes.
//!
//! The nodes it rewrites, as the TypeScript visitor (which does not visit inside a node it matched):
//! - `require("x")` and `import("x")` calls with one string literal argument;
//! - `import x = require("x")`;
//! - import and export declarations with a string literal module specifier;
//! - import types (`import("x")`) with a non-empty string literal.
//!
//! A specifier that does not resolve, or resolves into `node_modules` (an external library), is left as it is.

use rbxts_project::{ModuleResolver, nodepath};
use tsrs_ast::{Kind, Node, SourceFile};
use tsrs_core::{CompilerOptions, JsxEmit, P, ResolutionMode, tspath};

/// One rewrite: the byte range of a string literal token (quotes included) and its new text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rewrite {
    /// Where the literal starts (a byte offset).
    pub start: u32,
    /// Where it ends.
    pub end: u32,
    /// Its new text, quotes included.
    pub text: String,
}

/// `transformPaths(program, {})` for one program: its options, and a module resolver with them
/// (`ts.resolveModuleName` with `ts.sys`).
pub struct TransformPaths {
    resolver: Option<ModuleResolver>,
    implicit_extensions: Vec<&'static str>,
}

impl TransformPaths {
    /// `cwd`: the program's current directory, against which relative options resolve.
    pub fn new(options: P<CompilerOptions>, cwd: &str) -> TransformPaths {
        // getImplicitExtensions
        let mut implicit_extensions = vec![".ts", ".d.ts"];
        let allow_js = options.allow_js.is_true();
        let allow_jsx = options.jsx != JsxEmit::None;
        if allow_js {
            implicit_extensions.push(".js");
        }
        if allow_jsx {
            implicit_extensions.push(".tsx");
        }
        if allow_js && allow_jsx {
            implicit_extensions.push(".jsx");
        }
        if options.resolve_json_module.is_true() {
            implicit_extensions.push(".json");
        }
        // `if (!compilerOptions.baseUrl && !compilerOptions.paths) return sourceFile;`
        let active = !options.base_url.is_empty() || options.paths.is_some();
        TransformPaths { resolver: active.then(|| ModuleResolver::new(options, cwd)), implicit_extensions }
    }

    /// Whether it rewrites anything at all (the options set `baseUrl` or `paths`).
    pub fn is_active(&self) -> bool {
        self.resolver.is_some()
    }

    /// The rewrites of `file`, whose specifiers resolve as imports in `file_name` (the declaration's source file for
    /// an emitted declaration: TypeScript's declaration tree keeps its source's name), in text order.
    pub fn rewrites(&self, file: P<SourceFile>, file_name: &str) -> Vec<Rewrite> {
        let Some(resolver) = &self.resolver else { return Vec::new() };
        let text = file.get().text();
        let file_dir = tspath::normalize_path(nodepath::dirname(file_name));
        let mut literals = Vec::new();
        collect_specifiers(file.as_node(), &mut literals);
        let mut out: Vec<Rewrite> = literals
            .into_iter()
            .filter_map(|literal| {
                let path = self.update(resolver, literal.text(), file_name, &file_dir)?;
                let start = tsrs_scanner::skip_trivia(text, literal.pos().expect_source("a parsed literal"));
                Some(Rewrite {
                    start,
                    end: literal.end().expect_source("a parsed literal"),
                    text: print_string_literal(&path),
                })
            })
            .collect();
        out.sort_by_key(|r| r.start);
        out
    }

    /// `update`: the new specifier, or `None` to keep the original.
    fn update(&self, resolver: &ModuleResolver, module_name: &str, file_name: &str, file_dir: &str) -> Option<String> {
        // rbxts-rs: an unresolved specifier whose first failed lookup is a URL is rewritten to that URL in
        // roblox-ts; a lookup location is a file path, never a URL with a host, so it is always kept here
        let resolved = resolver.resolve(module_name, file_name, ResolutionMode::None)?;
        if resolved.is_external_library_import {
            return None;
        }
        let resolved_file_name = resolved.file_name;
        let module_dir = nodepath::dirname(&resolved_file_name);
        let relative = nodepath::relative(file_dir, module_dir);
        let mut p = tspath::normalize_path(&nodepath::join(&[&relative, nodepath::basename(&resolved_file_name)]));
        if let Some(extension) = extension_from_path(&resolved_file_name)
            && self.implicit_extensions.contains(&extension)
        {
            p.truncate(p.len() - extension.len());
        }
        if p.is_empty() {
            return None;
        }
        Some(if p.starts_with('.') { p } else { format!("./{p}") })
    }
}

/// TypeScript's `extensionFromPath` (`tryGetExtensionFromPath`): the resolved file's extension.
fn extension_from_path(path: &str) -> Option<&'static str> {
    const EXTENSIONS: [&str; 12] =
        [".d.ts", ".d.mts", ".d.cts", ".mjs", ".mts", ".cjs", ".cts", ".ts", ".js", ".tsx", ".jsx", ".json"];
    EXTENSIONS.into_iter().find(|e| path.ends_with(e))
}

/// The string literal a matched node's specifier is, when the node is one the transformer rewrites.
fn specifier_of(node: P<Node>) -> Option<P<Node>> {
    let string = |n: Option<P<Node>>| n.filter(|n| n.kind() == Kind::StringLiteral);
    match node.kind() {
        Kind::CallExpression => {
            let call = node.as_call_expression();
            let arguments = call.arguments.nodes();
            let [argument] = arguments else { return None };
            let callee = call.expression;
            let is_require = callee.kind() == Kind::Identifier && callee.text() == "require";
            let is_import = callee.kind() == Kind::ImportKeyword;
            if is_require || is_import { string(Some(*argument)) } else { None }
        }
        Kind::ExternalModuleReference => string(Some(node.as_external_module_reference().expression)),
        Kind::ImportDeclaration => string(Some(node.as_import_declaration().module_specifier)),
        Kind::ExportDeclaration => string(node.as_export_declaration().module_specifier),
        Kind::ImportType => {
            let argument = node.as_import_type_node().argument;
            if argument.kind() != Kind::LiteralType {
                return None;
            }
            string(Some(argument.as_literal_type_node().literal)).filter(|literal| !literal.text().is_empty())
        }
        _ => None,
    }
}

/// An import type the visitor matched without a specifier to rewrite (not a string literal argument, or an empty
/// one): TypeScript returns it as it is, without visiting inside it.
fn matched_without_specifier(node: P<Node>) -> bool {
    node.kind() == Kind::ImportType
}

/// The specifier literals of the nodes the visitor rewrites, in tree order, without looking inside a matched node.
fn collect_specifiers(node: P<Node>, out: &mut Vec<P<Node>>) {
    node.for_each_child(&mut |child| {
        match specifier_of(child) {
            Some(literal) => out.push(literal),
            None if matched_without_specifier(child) => {}
            None => collect_specifiers(child, out),
        }
        false
    });
}

/// The text TypeScript's printer gives a synthesized string literal: double quotes, `escapeString`, then every
/// non-ASCII UTF-16 unit as `\uXXXX` (`escapeNonAsciiString`; the declaration printer does not set
/// `neverAsciiEscape`).
pub fn print_string_literal(text: &str) -> String {
    let escaped = tsrs_printer::escape_string(text, tsrs_printer::QuoteChar::DoubleQuote);
    let mut out = String::with_capacity(escaped.len() + 2);
    out.push('"');
    for c in escaped.chars() {
        if c.is_ascii() {
            out.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04X}"));
            }
        }
    }
    out.push('"');
    out
}

/// Applies rewrites (sorted, not overlapping) to `text`.
pub fn apply(text: &str, rewrites: &[Rewrite]) -> String {
    let mut out = String::with_capacity(text.len() + rewrites.len() * 4);
    let mut at = 0;
    for rewrite in rewrites {
        out.push_str(&text[at..rewrite.start as usize]);
        out.push_str(&rewrite.text);
        at = rewrite.end as usize;
    }
    out.push_str(&text[at..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printed_literals() {
        assert_eq!(print_string_literal("./util/math"), "\"./util/math\"");
        assert_eq!(print_string_literal("./a\"b\\c"), "\"./a\\\"b\\\\c\"");
        assert_eq!(print_string_literal("./ü/😀"), "\"./\\u00FC/\\uD83D\\uDE00\"");
    }

    #[test]
    fn extensions() {
        assert_eq!(extension_from_path("E:/p/src/a.d.ts"), Some(".d.ts"));
        assert_eq!(extension_from_path("E:/p/src/a.ts"), Some(".ts"));
        assert_eq!(extension_from_path("E:/p/src/a.json"), Some(".json"));
        assert_eq!(extension_from_path("E:/p/src/a.luau"), None);
    }
}
