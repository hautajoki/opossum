-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local left = TS.import(script, game:GetService("ReplicatedStorage"), "left").left
local right = TS.import(script, game:GetService("ReplicatedStorage"), "right").right
local _src = TS.import(script, game:GetService("ReplicatedStorage"), "common")
local make = _src.make
local part = _src.part
print(left().size, right(), make(3).kind, part().Name)
