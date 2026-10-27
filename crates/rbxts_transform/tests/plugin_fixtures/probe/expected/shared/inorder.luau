-- Compiled with roblox-ts v3.0.0
-- the claimed node through compile_in_order: lowered as roblox-ts lowers it, once
local a = wrap(ProbeInOrder.new(1))
local _b = maybeInOrder
if _b ~= nil then
	_b = wrap(_b:io(2))
end
local b = _b
local c = wrap(getInOrder():io(3))
local d = wrap(probeInOrderFn(4))
local _ = probeVoidInOrder
local e = wrap(nil)
return {
	a = a,
	b = b,
	c = c,
	d = d,
	e = e,
}
