-- Compiled with roblox-ts v3.0.0
-- A module augmentation's resolution counts like an import's (tests/fixtures.rs, `junction_augmentation`): this file,
-- first in root order, augments `@rbxts/zeta`, so `b.ts`'s import of `@rbxts/alpha` goes through `zeta`.
local a = 1
return {
	a = a,
}
