-- Compiled with roblox-ts v3.0.0
-- property_key: every access whose receiver has `__probeKey`, in every position
local r1 = keyed.k_a
local r2 = keyed.k_b
keyed.k_a = 1
keyed.k_b += 2
keyed.k_s ..= "x"
keyed.k_a += 1
local _original = keyed.k_a
keyed.k_a += 1
local r3 = _original
keyed.k_b += 1
local r4 = keyed.k_b
keyed.k_opt = nil
local r5 = keyed:k_m(1)
local r6 = keyed.k_f(2)
local _r7 = maybeKeyed
if _r7 ~= nil then
	_r7 = _r7.k_a
end
local r7 = _r7
local _r8 = maybeKeyed
if _r8 ~= nil then
	_r8 = _r8:k_m(3)
end
local r8 = _r8
local _self = keyed
local _r9 = _self.k_m
if _r9 ~= nil then
	_r9 = _r9(_self, 4)
end
local r9 = _r9
keyed.k_a, keyed.k_b = 1, sideEffect()
local _binding = {
	a = 5,
}
keyed.k_a = _binding.a
local r10 = keyed["key with space"]
keyed["key with space"] = 3
local _original_1 = keyed["key with space"]
keyed["key with space"] += 1
local r11 = _original_1
local r12 = getKeyed().k_inner.k_inner.k_a
for _, _v in { 1, 2 } do
	keyed.k_a = _v
	print(keyed.k_a)
end
if keyed.k_a == nil then
	keyed.k_a = 4
end
local r13 = keyed.same
local r14 = keyed["key with space"]
return {
	r1 = r1,
	r2 = r2,
	r3 = r3,
	r4 = r4,
	r5 = r5,
	r6 = r6,
	r7 = r7,
	r8 = r8,
	r9 = r9,
	r10 = r10,
	r11 = r11,
	r12 = r12,
	r13 = r13,
	r14 = r14,
}
