-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- Package imports through linked directories (tests/fixtures.rs, `junction`): `@rbxts/services` from a shared
-- node_modules, `@rbxts/greeter` from `packages/greeter`, outside any node_modules.
local Workspace = TS.import(script, game:GetService("ReplicatedStorage"), "include", "node_modules", "@rbxts", "services").Workspace
local greet = TS.import(script, game:GetService("ReplicatedStorage"), "include", "node_modules", "@rbxts", "greeter").greet
local function hello(name)
	return greet(name)
end
local gravity = Workspace.Gravity
return {
	hello = hello,
	gravity = gravity,
}
