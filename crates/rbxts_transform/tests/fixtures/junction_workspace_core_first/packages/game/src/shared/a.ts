// The isolated workspace of `junction_workspace`, importing `core` first (tests/fixtures.rs,
// `junction_workspace_core_first`): this file's resolution of `@rbxts/services` still comes before the one in core's
// declarations, so imports go through game's own link.
import { core } from "@rbxts/core";
import { Workspace } from "@rbxts/services";

export const x = core(Workspace);
