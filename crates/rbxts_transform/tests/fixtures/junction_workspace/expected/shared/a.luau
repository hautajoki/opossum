-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- A bun / pnpm isolated workspace (tests/fixtures.rs, `junction_workspace`): `game` and its sibling `core` both link
-- `@rbxts/services` from the workspace's `node_modules/.store`. TypeScript 5 meets game's link first, through this
-- import, and core's link later, through core's declarations; imports go through the first.
local Workspace = TS.import(script, game:GetService("ReplicatedStorage"), "include", "node_modules", "@rbxts", "services").Workspace
local core = TS.import(script, game:GetService("ReplicatedStorage"), "include", "node_modules", "@rbxts", "core").core
local x = core(Workspace)
return {
	x = x,
}
