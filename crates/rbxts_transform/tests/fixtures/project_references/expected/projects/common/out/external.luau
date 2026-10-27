-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local exports = {}
exports.add = TS.import(script, game:GetService("ReplicatedStorage"), "external", "helper").add
return exports
