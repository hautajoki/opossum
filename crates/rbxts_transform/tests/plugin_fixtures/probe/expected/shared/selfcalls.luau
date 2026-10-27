-- Compiled with roblox-ts v3.0.0
-- compile_expr of the claimed call itself: lowered as roblox-ts lowers it, on the receiver already compiled
local n = 0
local s1 = wrap(getChain():s(sideEffect()))
local _s2 = maybeChain
if _s2 ~= nil then
	local _fn = _s2.s
	n += 1
	_s2 = wrap(_fn(_s2, n))
end
local s2 = _s2
local s3 = wrap(getChain().inner:s(2))
local _s4 = probeSelf
if _s4 ~= nil then
	_s4 = wrap(_s4(3))
end
local s4 = _s4
local s5 = wrap(probeSelf(4))
return {
	s1 = s1,
	s2 = s2,
	s3 = s3,
	s4 = s4,
	s5 = s5,
}
