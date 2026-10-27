# rbxts_plugin

The plugin API of rbxts-rs: the traits a compiled-in plugin implements, and the services the compiler offers it
(DESIGN §11, with the build lifecycle of §14). `flamework_rs` is built on it; the compiler (`rbxts_transform`)
implements the backend traits.

- `Plugin` (factory), `PluginInstance` (per checker), `Plugin::FilePart` (per file), `Plugin::State` (persistent).
- `Cx` / `SetupCx` / `FinishCx`: thin wrappers over the compiler's `CxBackend` / `SetupBackend` / `FinishBackend`.
- `Interest`: which nodes an instance wants; the compiler never calls a plugin for anything else.
- `erased`: `erase(plugin)` for the compiler's plugin list (`Box<dyn DynPlugin>`).
- `escape_luau_string` / `text_literal`: plain text to Luau string literals (late strings use the same escaping).

## Lifecycle

```text
                       ┌──────────── one build (Clean | Incremental | WatchRebuild{generation}) ─────────────┐
 committed State ──clone──▶ working State
                       │
 Shared kept? ──yes──▶ rebuild_start(shared, state, changes) ──KeepShared──┐
 (WatchRebuild only; kept only with the committed state it was made from)  │
     │ no: Clean, Incremental, last build did not commit,         RerunSetup │
     │     or a build input read by setup changed                            │
     ▼                                                             ▼        │
 setup(SetupCx, &mut state) ──▶ Arc<Shared>  ◀────────────────────────────┘
     │   reads files/env/globs only via cx (build inputs: watched + hashed)
     ▼
 per checker thread: instance(Arc<Shared>, CheckerInfo) ──▶ interest(&mut Interest)
     │   per file of the affected closure, in transform order on that checker:
     │     begin_file ─▶ claim hooks (call, property_key, assignment, expression, statement)
     │                   (cx.env / cx.record_glob: inputs of this file)
     │                ─▶ class (static-block position) ─▶ end_file ──▶ FilePart
     ▼
 finish(shared, &mut state, parts of ALL current files sorted by FileIndex, FinishCx)
     │   resolve_late(key, text) for the keys of every current file · write_artifact · write_state_file
     │   remove_artifact · remove_empty_dir · record_glob · read_dir / exists / read_file · watch_dir · warn_at
     ▼
 compiler: late-string key check ─▶ render ─▶ commit (only when the build has no errors):
     1 Luau + include files   2 plugin artifacts   3 stale outputs/artifacts deleted
     4 plugin state files, then the compiler's incremental state ──▶ working State becomes committed,
                                                                     and the Shared is kept with it
 no commit (errors or Fatal) ──▶ working State and the Shared made by this build are dropped
```

- A build with errors writes nothing and keeps the committed state. The compiler keeps a `Shared` only together
  with the state it was made from: a `Shared` made by a build that does not commit (errors or `Fatal`) is dropped,
  and the next build runs `setup` again on the committed state. In watch mode a `Fatal` is reported and the
  watcher keeps running.
- `rebuild_start` runs only on watch rebuilds that have a kept `Shared`, never on `Clean` or `Incremental` builds.
- `FilePart`s and `State` outlive programs: no handles (`P<…>`, program text) and no `FileIndex` in them; key by
  `cx.file().path`.
- Late-string values may change between builds; the compiler re-renders cached files whose values changed, but
  plugins should keep committed values stable.
- Instances live for one build: their caches may key on nodes. Cross-file state goes in `FilePart`s only.
- A plugin never writes files itself; it queues them through `FinishCx`.
- `encode_file_part` / `decode_file_part` (default `None`) let the compiler keep file parts on disk for
  incremental builds in a new process; without them those files are transformed again.

## Porting from today's TypeScript plugin API

| `@flamework-experimental/transformer-plugin` / TS transformers | rbxts_plugin / flamework_rs |
| --- | --- |
| `registerPlugin(api => …)`, `registerMacroType(id, fn)` | `impl flamework_rs::MacroType` + `.macro_type("id", value)` in the bin's `main`; a generic TS transformer becomes `impl Plugin` |
| a ts-patch `plugins` entry `{ transform }` | `PluginInfo::tsconfig_transform`; options from `SetupCx::tsconfig_entry` or Rust fields set in `main` |
| `api.options` / `api.version` | fields of the plugin value / `PluginInfo::version` (`flamework_rs::VERSION`) |
| `MacroContext.hoist(expr, name)` / `hoistStatement(s)` | `cx.hoist(expr, name)` / `cx.insert_before_root_statement([s])` |
| `MacroContext.error(msg)` / `warning(msg)` | `return Err(cx.error(node, msg))` / `cx.warning(node, msg)` |
| `Diagnostics.error` + `catchDiagnostic` | `PResult` / `Abort`; `cx.report(PluginDiagnostic { related, .. })` for related info |
| `ctx.fileName` | `cx.file().path` / `cx.file().relative_path` |
| visiting every node and matching a call | `Interest::calls_by_symbol` + `classify_symbol`, `calls_by_receiver`, `calls_by_callee_marker` |
| visiting every node of a kind | `Interest::expressions(kind, tag)` / `statements(kind, tag)` (slow path) |
| emitting TS for roblox-ts to compile | return Luau; lower user code with `cx.compile_expr` / `compile_in_order` / `compile_call` / `compile_object_literal` / `compile_binary` |
| `factory.updateCallExpression(…, args)` | `cx.compile_call(call, Callee::Original, vec![Arg::Original(n), Arg::Luau(e), Arg::Nil])` |
| the chain IIFE around a receiver | `cx.compile_receiver(call)` (nil-checked by the chain lowering) |
| `typeToTypeNode` / casts so the re-check passes | gone: nothing is re-checked |
| an import whose only value uses the rewrite removed (TS elided it after the rewrite) | `cx.request_import_elision(alias)`; `cx.mark_import_used(alias)` for the opposite |
| the Type facade (`isUnion`, `getFields`, `isLiteral`, …) | `flamework_rs::MacroCx` queries (`constituents`, `fields`, `literal`, …); anything else `cx.checker()` (raw tsrs) |
| `factory.expr.string(s)` | `rbxts_plugin::text_literal(s)` (plain text, escaped); `rbxts_luau::create::*` for the rest |
| `factory.expr.identifier(n, true)` | `cx.temp_id(hint)` |
| `getJSDocTags` | `cx.jsdoc_tags(declaration)` / `cx.symbol_jsdoc_tags(symbol)` (`{@link}` symbols resolved) |
| `node.getStart()` / `getText()` / `getLineAndCharacterOfPosition` | `cx.utf16_pos(node)` / `cx.source_text(node)` / `cx.utf16_width(node)` |
| ids decided at transform time from shared mutable state | `cx.late_string(key)` now, `FinishCx::resolve_late(key, text)` in `finish` |
| `setTimeout(saveArtifacts)`, `fs.writeFileSync` | `FinishCx::write_artifact` / `write_state_file` (one write phase) |
| reading config with `fs` / `process.env` | `SetupCx::read_input` / `env` (recorded as build inputs) |
| `process.env` at transform time | `cx.env(name)` (recorded as an input of the file) |
| `glob.sync(pattern, { root })` | `record_glob(root, pattern, matcher)` on `SetupCx`, `Cx` or `FinishCx`; the compiler matches again in watch |
| `fs.rmdirSync(folder)` of an emptied artifact folder | `FinishCx::remove_empty_dir(folder)?` (the compiler removes folders its artifact diff empties by itself) |
| `fs.existsSync` / `readdirSync` / `readFileSync` while writing artifacts | `FinishCx::exists` / `read_dir` / `read_file` (recorded: watch re-runs `finish` when an answer changes) |
| a diagnostic `code` such as `" @scope/transformer"` | `Plugin::diagnostic_source` (per diagnostic: `PluginDiagnostic::with_source`) |
| `process.exit(1)` | `Err(Fatal::new(msg))` from `setup` / `finish` |

## Tests

`tests/stamp.rs` runs a small plugin (`tests/support/stamp.rs`: JSDoc-tagged calls become string literals or late
strings) against a mock compiler (`tests/support/mock.rs`) over a real tsrs program (`tests/fixture`), across
checker counts and builds. `tests/contracts.rs` runs a misbehaving plugin (`tests/support/rogue.rs`) to check the
contracts the compiler enforces: phases, `Err` without an error, unresolved late strings, no diagnostics from
`classify_symbol`. The mock implements each contract as the compiler must. `tests/defaults.rs` checks the default
implementations of the backend methods added for watch (`env`, `record_glob`, `remove_empty_dir`, `read_dir`,
`exists`, `read_file`), the default `encode_file_part` / `decode_file_part` and the diagnostic sources. The
compiler-side test of the real dispatch lives in `rbxts_transform`.
