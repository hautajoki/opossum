-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- Two links to one package (tests/fixtures.rs, `junction_two_links`): imports of either go through the link
-- TypeScript 5 met first, `@rbxts/zeta`.
local greet = TS.import(script, game:GetService("ReplicatedStorage"), "include", "node_modules", "@rbxts", "zeta").greet
local g2 = TS.import(script, game:GetService("ReplicatedStorage"), "include", "node_modules", "@rbxts", "zeta").greet
local x = greet("z") .. g2("a")
return {
	x = x,
}
