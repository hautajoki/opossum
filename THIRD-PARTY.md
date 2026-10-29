# Third-party code

opossum is MIT OR Apache-2.0 (LICENSE-MIT, LICENSE-APACHE). These parts come from elsewhere and keep their own terms:

- `vendor/tsrs/`: [tsrs](https://github.com/oxc-project/tsrs), the TypeScript checker in Rust. Apache-2.0, see its
  `LICENSE` and `NOTICE` (a derivative of TypeScript, Copyright (c) Microsoft Corporation). `opossum.patch` is our
  delta.
- `vendor/luau0-src/`: the luau0-src crate and the Luau sources it carries. MIT, see its `LICENSE`. `opossum.patch` is
  our delta.
- `crates/luau/src/vm.cpp`: its require-by-string file navigator is ported from Luau's CLI (MIT, Copyright (c)
  2019-2025 Roblox Corporation).
- `runtime/`: the roblox-ts runtime library and the Promise implementation roblox-ts ships with it. MIT, see
  `runtime/LICENSE`.
- `crates/lower/`: the TypeScript lowering started from rbxts-rs, a Rust port of the
  [roblox-ts](https://github.com/roblox-ts/roblox-ts) compiler, and keeps roblox-ts's layout. roblox-ts is MIT,
  Copyright (c) roblox-ts (the same terms as `runtime/LICENSE`).
