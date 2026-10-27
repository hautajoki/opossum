// A file's resolutions come before those of the files it imports (tests/fixtures.rs, `junction_interleaved_imports`):
// this file resolves `@rbxts/hub` and `@rbxts/zeta` before the hub's declarations resolve `@rbxts/alpha`, so imports of
// the greeter go through `zeta`, the first link TypeScript 5 met.
import { h } from "@rbxts/hub";
import { greet } from "@rbxts/zeta";

export const z = greet("z");
export const hh = h;
