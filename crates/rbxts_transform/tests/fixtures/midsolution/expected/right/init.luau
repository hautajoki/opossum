-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local _src = TS.import(script, game:GetService("ReplicatedStorage"), "common")
local bump = _src.bump
local count = _src.count
local function right()
	bump()
	return count
end
return {
	right = right,
}
