-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- A server script using the fixture modules.
local plain = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "conditional").plain
local counter = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "generators").counter
print(plain(true, 1, 2), counter(1))
