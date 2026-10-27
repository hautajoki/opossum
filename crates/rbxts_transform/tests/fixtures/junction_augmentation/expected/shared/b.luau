-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- Imports the greeter through `@rbxts/alpha` (see a.ts).
local greet = TS.import(script, game:GetService("ReplicatedStorage"), "include", "node_modules", "@rbxts", "zeta").greet
local b = greet("b")
return {
	b = b,
}
