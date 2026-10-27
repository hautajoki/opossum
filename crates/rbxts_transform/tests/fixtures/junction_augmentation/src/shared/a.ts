// A module augmentation's resolution counts like an import's (tests/fixtures.rs, `junction_augmentation`): this file,
// first in root order, augments `@rbxts/zeta`, so `b.ts`'s import of `@rbxts/alpha` goes through `zeta`.
declare module "@rbxts/zeta" {
	export const extra: number;
}

export const a = 1;
