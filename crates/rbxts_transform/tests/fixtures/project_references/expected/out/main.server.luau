-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local add = TS.import(script, game:GetService("ReplicatedStorage"), "common", "native").add
local config = TS.import(script, game:GetService("ReplicatedStorage"), "external", "config")
local addExternal = TS.import(script, game:GetService("ReplicatedStorage"), "external", "helper").add
local addFromReference = TS.import(script, game:GetService("ReplicatedStorage"), "common", "external").add
local _src = TS.import(script, game:GetService("ReplicatedStorage"), "shared")
local accumulate = _src.accumulate
local common = _src.common
local Counter = _src.Counter
local NamedCounter = _src.NamedCounter
local greet = TS.import(script, game:GetService("ServerScriptService"), "main", "greet").greet
local counter = NamedCounter.new()
local named = counter
print(greet(named.name), TS.instanceof(counter, Counter), TS.instanceof(counter, common.Counter), counter:increment())
print(add(4, 5), addExternal(config.value), addFromReference(config.value))
accumulate({ 2, 3 }, function(value)
	return print(value, common.total)
end)
