// Enum values that are not finite numbers. rbxts-rs writes infinities as `math.huge` (and NaN as `0 / 0`); stock
// roblox-ts writes `Infinity`, an undefined global in Luau (a known, accepted deviation), and overflows its stack on
// a NaN member (`luau.number(NaN)` recurses), so NaN is left out here.
export enum NonFinite {
	Inf = 1 / 0,
	NegInf = -1 / 0,
	Large = 1e300 * 10,
	Finite = 2,
}
