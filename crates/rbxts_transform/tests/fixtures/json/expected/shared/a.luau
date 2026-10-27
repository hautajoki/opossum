-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local data = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "data")
local list = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "cfg", "list")
local _data_json = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "data")
local speed = _data_json.speed
local nested = _data_json.nested
local settings = TS.import(script, game:GetService("ReplicatedStorage"), "config", "settings")
-- exported functions first: TypeScript 5 lists them first in the export table, TypeScript 7 in source order
local load = TS.async(function()
	local lazy = TS.await(TS.Promise.new(function(resolve)
		resolve(TS.import(script, game:GetService("ReplicatedStorage"), "shared", "data"))
	end))
	return lazy.default.name
end)
local first = list[1] + data.speed
local named = speed + (if nested.on then 1 else 0)
local gravity = settings.gravity
local tagCount = #data.tags
return {
	load = load,
	first = first,
	named = named,
	gravity = gravity,
	tagCount = tagCount,
}
