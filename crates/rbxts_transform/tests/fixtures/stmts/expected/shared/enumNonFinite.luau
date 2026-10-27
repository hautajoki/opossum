-- Compiled with roblox-ts v3.0.0
-- Enum values that are not finite numbers. rbxts-rs writes infinities as `math.huge` (and NaN as `0 / 0`); stock
-- roblox-ts writes `Infinity`, an undefined global in Luau (a known, accepted deviation), and overflows its stack on
-- a NaN member (`luau.number(NaN)` recurses), so NaN is left out here.
local NonFinite
do
	local _inverse = {}
	NonFinite = setmetatable({}, {
		__index = _inverse,
	})
	NonFinite.Inf = math.huge
	_inverse[math.huge] = "Inf"
	NonFinite.NegInf = -math.huge
	_inverse[-math.huge] = "NegInf"
	NonFinite.Large = 1e+301
	_inverse[1e+301] = "Large"
	NonFinite.Finite = 2
	_inverse[2] = "Finite"
end
return {
	NonFinite = NonFinite,
}
