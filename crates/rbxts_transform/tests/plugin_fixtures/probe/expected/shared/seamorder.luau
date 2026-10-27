-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local _runtime = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "runtime")
local other = _runtime.other
local impl = _runtime.impl
-- a leading comment with no blank line after it stays with its statement, below the plugin imports
local after = { impl("b", 2), other }
return {
	after = after,
}
