-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- Leading comment.
local helper = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "helper").helper
local function f(a, b)
	if a > 1 then
		return b
	end
	return `{a}{b}`
end
local value = helper(1) + 2
return {
	f = f,
	value = value,
}
