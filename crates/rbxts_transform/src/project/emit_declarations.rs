//! The declaration emit of `Project/functions/compileFiles.ts`: in a declaration build (`declaration` or
//! `composite`), every file the build transformed gets the `.d.ts` (and `.d.ts.map`) TypeScript emits for it, after
//! roblox-ts's `afterDeclarations` transformers ([`transform_type_reference_directives`], [`TransformPaths`]).
//! Written with the Luau outputs, all or nothing: an emit error writes neither.
//!
//! rbxts-rs: tsrs's emitter (`Program::emit`, declarations only) prints the declaration; roblox-ts's transformers then
//! rewrite its text (see [`super::transformers::transform_paths`]). A rewritten specifier changes the length of its
//! line, so the declaration map's later mappings on that line move with it. TypeScript 5 maps the synthesized
//! specifier itself differently (and TypeScript 7 maps some declarations, constructors among them, differently from
//! TypeScript 5): declaration maps are TypeScript 7's, not byte-equal to stock roblox-ts's.

use std::sync::Mutex;

use tsrs_ast::{Diagnostic, SourceFile};
use tsrs_compiler::{Context, EmitOnly, EmitOptions, Program, WriteFileData};
use tsrs_core::P;

use super::transform_declaration_file::parse;
use super::transformers::transform_paths::{Rewrite, TransformPaths, apply};
use super::transformers::transform_type_reference_directives;

/// The declaration outputs of `files` (path, text), sorted by path, and the emit's diagnostics.
pub fn emit_declarations(
    program: &'static Program,
    ctx: &Context,
    files: &[P<SourceFile>],
    paths: &TransformPaths,
) -> (Vec<(String, String)>, Vec<P<Diagnostic>>) {
    if files.is_empty() {
        return (Vec::new(), Vec::new());
    }
    // (output path, text, source file name)
    let written: Mutex<Vec<(String, String, Option<String>)>> = Mutex::new(Vec::new());
    let write = |file_name: &str, text: &str, data: &mut WriteFileData| -> Result<(), String> {
        let source = data.source_file.map(|f| f.get().file_name().to_owned());
        written.lock().unwrap().push((file_name.to_owned(), text.to_owned(), source));
        Ok(())
    };
    let result = program.emit(
        ctx,
        &EmitOptions {
            target_source_files: Some(files.to_vec()),
            emit_only: EmitOnly::EmitOnlyDts,
            force_emit: false,
            write_file: Some(&write),
        },
    );
    let mut written = written.into_inner().unwrap();
    written.sort_by(|a, b| a.0.cmp(&b.0));

    // the declarations first, so each map knows how its declaration's lines moved
    let mut shifts: Vec<(String, Vec<LineShift>)> = Vec::new();
    for (path, text, source) in &mut written {
        if !path.ends_with(".d.ts") {
            continue;
        }
        let file = parse(path, text);
        let mut rewrites = transform_type_reference_directives::rewrites(file);
        if let Some(source) = source.as_deref() {
            rewrites.extend(paths.rewrites(file, source));
        }
        if rewrites.is_empty() {
            continue;
        }
        rewrites.sort_by_key(|r| r.start);
        shifts.push((format!("{path}.map"), line_shifts(text, &rewrites)));
        *text = apply(text, &rewrites);
    }
    let outputs = written
        .into_iter()
        .map(|(path, text, _)| {
            let text = match shifts.iter().find(|(map, _)| *map == path) {
                Some((_, shifts)) => shift_map(&text, shifts),
                None => text,
            };
            (path, text)
        })
        .collect();
    (outputs, result.diagnostics)
}

/// A rewrite as the map sees it: on generated line `line`, mappings at or after UTF-16 column `after` move by `delta`.
#[derive(Clone, Copy, Debug)]
struct LineShift {
    line: u32,
    after: u32,
    delta: i64,
}

fn utf16_len(text: &str) -> i64 {
    text.encode_utf16().count() as i64
}

/// Where each rewrite of `text` sits, as line and UTF-16 columns.
fn line_shifts(text: &str, rewrites: &[Rewrite]) -> Vec<LineShift> {
    rewrites
        .iter()
        .map(|rewrite| {
            let before = &text[..rewrite.start as usize];
            let line = before.matches('\n').count() as u32;
            let line_start = before.rfind('\n').map_or(0, |i| i + 1);
            let end_column = utf16_len(&text[line_start..rewrite.end as usize]) as u32;
            let old = &text[rewrite.start as usize..rewrite.end as usize];
            LineShift { line, after: end_column, delta: utf16_len(&rewrite.text) - utf16_len(old) }
        })
        .collect()
}

/// The source map `json` with its mappings moved by `shifts`.
fn shift_map(json: &str, shifts: &[LineShift]) -> String {
    const KEY: &str = "\"mappings\":\"";
    let Some(start) = json.find(KEY).map(|i| i + KEY.len()) else { return json.to_owned() };
    let Some(length) = json[start..].find('"') else { return json.to_owned() };
    let mappings = &json[start..start + length];
    let mut decoder = tsrs_sourcemap::decode_mappings(mappings);
    let segments: Vec<tsrs_sourcemap::Mapping> = decoder.values().collect();
    if decoder.error().is_some() {
        return json.to_owned();
    }
    let moved: Vec<tsrs_sourcemap::Mapping> = segments
        .into_iter()
        .map(|mut segment| {
            let delta: i64 = shifts
                .iter()
                .filter(|s| s.line == segment.generated_line && segment.generated_character >= s.after)
                .map(|s| s.delta)
                .sum();
            segment.generated_character = (i64::from(segment.generated_character) + delta).max(0) as u32;
            segment
        })
        .collect();
    format!("{}{}{}", &json[..start], encode_mappings(&moved), &json[start + length..])
}

/// Encodes mappings as the source map generator does: `;` per generated line, `,` between a line's segments, each
/// field relative to the one before it (the generated column within its line).
fn encode_mappings(segments: &[tsrs_sourcemap::Mapping]) -> String {
    let mut out = String::new();
    let (mut line, mut column, mut source, mut source_line, mut source_column, mut name) =
        (0u32, 0i64, 0i64, 0i64, 0i64, 0i64);
    let mut first_in_line = true;
    for segment in segments {
        while line < segment.generated_line {
            out.push(';');
            line += 1;
            column = 0;
            first_in_line = true;
        }
        if !first_in_line {
            out.push(',');
        }
        first_in_line = false;
        vlq(&mut out, i64::from(segment.generated_character) - column);
        column = i64::from(segment.generated_character);
        if let (Some(index), Some(l), Some(c)) =
            (segment.source_index.get(), segment.source_line, segment.source_character)
        {
            vlq(&mut out, i64::from(index) - source);
            source = i64::from(index);
            vlq(&mut out, i64::from(l) - source_line);
            source_line = i64::from(l);
            vlq(&mut out, i64::from(c) - source_column);
            source_column = i64::from(c);
            if let Some(n) = segment.name_index.get() {
                vlq(&mut out, i64::from(n) - name);
                name = i64::from(n);
            }
        }
    }
    out
}

/// Base64 VLQ.
fn vlq(out: &mut String, value: i64) {
    const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut rest = if value < 0 { ((-value) << 1) | 1 } else { value << 1 };
    loop {
        let mut digit = rest & 31;
        rest >>= 5;
        if rest > 0 {
            digit |= 32;
        }
        out.push(BASE64[digit as usize] as char);
        if rest == 0 {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mappings_round_trip() {
        let mappings = ";AAEA,OAAO,EAAS,MAAM,EAAE,MAAM,WAAW,CAAC;AAC1C,OAAO,KAAK;;IAGA,OAAO,CAAC,QAAQ,CAAC,KAAK";
        let segments: Vec<tsrs_sourcemap::Mapping> = tsrs_sourcemap::decode_mappings(mappings).values().collect();
        assert_eq!(encode_mappings(&segments), mappings);
    }

    #[test]
    fn shifts_follow_rewrites() {
        let text = "x;\nimport { a } from \"a\";\n";
        let start = text.find("\"a\"").unwrap() as u32;
        let rewrites = [Rewrite { start, end: start + 3, text: "\"./a\"".into() }];
        let shifts = line_shifts(text, &rewrites);
        assert_eq!((shifts[0].line, shifts[0].after, shifts[0].delta), (1, 21, 2));
        // a segment at column 21 (`;`) moves to 23; one at column 0 stays
        let map = "{\"version\":3,\"mappings\":\";AAAA,qBAAqB\"}";
        assert_eq!(shift_map(map, &shifts), "{\"version\":3,\"mappings\":\";AAAA,uBAAqB\"}");
    }
}
