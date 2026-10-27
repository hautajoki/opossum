-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- A file's resolutions come before those of the files it imports (tests/fixtures.rs, `junction_interleaved_imports`):
-- this file resolves `@rbxts/hub` and `@rbxts/zeta` before the hub's declarations resolve `@rbxts/alpha`, so imports of
-- the greeter go through `zeta`, the first link TypeScript 5 met.
local h = TS.import(script, game:GetService("ReplicatedStorage"), "include", "node_modules", "@rbxts", "hub").h
local greet = TS.import(script, game:GetService("ReplicatedStorage"), "include", "node_modules", "@rbxts", "zeta").greet
local z = greet("z")
local hh = h
return {
	z = z,
	hh = hh,
}
