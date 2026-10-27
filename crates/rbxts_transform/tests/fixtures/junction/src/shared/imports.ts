// Package imports through linked directories (tests/fixtures.rs, `junction`): `@rbxts/services` from a shared
// node_modules, `@rbxts/greeter` from `packages/greeter`, outside any node_modules.
import { Workspace } from "@rbxts/services";
import { greet } from "@rbxts/greeter";
import type { Greeting } from "@rbxts/greeter";

export function hello(name: string): Greeting {
	return greet(name);
}

export const gravity = Workspace.Gravity;
