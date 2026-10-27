-- Compiled with roblox-ts v3.0.0
-- object literal methods in `this: void` slots: TypeScript 5 gives them the slot's `this`, so roblox-ts accepts them
-- as callbacks (no "Attempted to assign method where non-method was expected"); TypeScript 7 alone would not
local t = {
	f = function() end,
}
local tv = t
local u = {
	f = function(n) end,
}
local uv = u
return {
	tv = tv,
	uv = uv,
}
