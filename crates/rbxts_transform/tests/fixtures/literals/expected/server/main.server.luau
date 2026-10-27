-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local plain = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "objects").plain
local simple = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "arrays").simple
print(plain, simple)
