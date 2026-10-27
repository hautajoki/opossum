// A bun / pnpm isolated workspace (tests/fixtures.rs, `junction_workspace`): `game` and its sibling `core` both link
// `@rbxts/services` from the workspace's `node_modules/.store`. TypeScript 5 meets game's link first, through this
// import, and core's link later, through core's declarations; imports go through the first.
import { Workspace } from "@rbxts/services";
import { core } from "@rbxts/core";

export const x = core(Workspace);
