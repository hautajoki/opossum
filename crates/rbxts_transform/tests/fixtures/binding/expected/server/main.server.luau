-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local alpha = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "variables").alpha
local objectParameter = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "parameters").objectParameter
local swap = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "assignments").swap
local strings = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "iterables").strings
TS.try(function()
	print(alpha, objectParameter({
		a = 1,
	}), swap(), strings())
end, function(e)
	print(e)
end)
