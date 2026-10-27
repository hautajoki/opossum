-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- A package imported by relative path while it is also a dependency linked into node_modules (tests/fixtures.rs,
-- `junction_relative_dependency`): no resolution goes through the link, so TypeScript 5 knows of none and the import
-- keeps the package's own path.
local greet = TS.import(script, game:GetService("ReplicatedStorage"), "packages", "greeter", "index").greet
local x = greet("r")
return {
	x = x,
}
