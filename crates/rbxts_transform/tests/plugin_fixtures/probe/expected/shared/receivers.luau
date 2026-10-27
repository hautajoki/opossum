-- Compiled with roblox-ts v3.0.0
local r1 = marked:probed(1)
local _r2 = maybeMarked
if _r2 ~= nil then
	_r2 = _r2:probed(2)
end
local r2 = _r2
local r3 = unmarked:probeMethod(3)
local c1 = callable(4, 1)
return {
	r1 = r1,
	r2 = r2,
	r3 = r3,
	c1 = c1,
}
