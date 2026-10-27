-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local _impa = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "impa")
local u = _impa.u
local t = _impa.t
local t_1 = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "impb").t
-- plugin imports, as Flamework's TS transformer adds them: one declaration per path, the paths in the order their
-- first import was asked for, each listing its imports in that order; TypeScript names a second `t` `t_1` in that
-- print order, so `./impa`'s `t` is `t` though `./impb`'s was asked for first
local i1 = u
local i2 = t_1
local i3 = t
local function later()
	return { t_1, u }
end
return {
	i1 = i1,
	i2 = i2,
	i3 = i3,
	later = later,
}
