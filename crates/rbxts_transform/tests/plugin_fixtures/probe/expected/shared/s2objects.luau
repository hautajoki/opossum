-- Compiled with roblox-ts v3.0.0
-- compile_object_literal: `a` renamed, `b` replaced (its value not evaluated), `e` appended; spreads and computed
-- keys kept
local key = "k"
local _object = {
	renamed = 1,
	b = "replaced",
}
for _k, _v in extra do
	_object[_k] = _v
end
_object["c d"] = 2
_object[key] = 3
_object.e = 5
local o1 = wrap(_object)
-- emitted in another order, evaluated in source order
local _field = sideEffect()
local o2 = {
	w = 4,
	z = sideEffect() + 1,
	x = _field,
	y = 2,
}
local b1 = "x" .. tostring(1)
local b2 = 1 + sideEffect()
return {
	o1 = o1,
	o2 = o2,
	b1 = b1,
	b2 = b2,
}
