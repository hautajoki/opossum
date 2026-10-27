-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- The same order for type reference directives (tests/fixtures.rs, `junction_interleaved_type_refs`): this file's
-- references to `@rbxts/hub` and `@rbxts/zeta` come before the hub's reference to `@rbxts/alpha`, so the import of
-- the greeter by relative path goes through `zeta`.
--/ <reference types="@rbxts/hub" />
--/ <reference types="@rbxts/zeta" />
local greet = TS.import(script, game:GetService("ReplicatedStorage"), "include", "node_modules", "@rbxts", "zeta").greet
local r = greet("r")
return {
	r = r,
}
