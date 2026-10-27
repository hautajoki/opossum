-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- The isolated workspace of `junction_workspace`, importing `core` first (tests/fixtures.rs,
-- `junction_workspace_core_first`): this file's resolution of `@rbxts/services` still comes before the one in core's
-- declarations, so imports go through game's own link.
local core = TS.import(script, game:GetService("ReplicatedStorage"), "include", "node_modules", "@rbxts", "core").core
local Workspace = TS.import(script, game:GetService("ReplicatedStorage"), "include", "node_modules", "@rbxts", "services").Workspace
local x = core(Workspace)
return {
	x = x,
}
