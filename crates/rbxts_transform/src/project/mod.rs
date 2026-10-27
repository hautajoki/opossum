//! Build steps of roblox-ts's `src/Project/functions` that run around the transform, one module per TypeScript
//! file.
//!
//! - [`copy_include`]: `copyInclude.ts` and `getIncludeFiles.ts`, the runtime library files copied into the
//!   project's include folder.
//! - [`emit_declarations`]: the declaration emit of `compileFiles.ts`.
//! - [`transform_declaration_file`]: `transformDeclarationFile.ts`, the declaration assets.
//! - [`transformers`]: `Project/transformers/builtin`, roblox-ts's own declaration transformers.
//! - [`write_transformed_files`]: the outputs of `--writeTransformedFiles`.

pub mod copy_include;
pub mod emit_declarations;
pub mod transform_declaration_file;
pub mod transformers;
pub mod write_transformed_files;
