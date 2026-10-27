// A base in another file than the interface extending it (item.ts).
export interface Card {
	title: string;
	// The members after `body` sit at byte offsets inside `Item`'s range in item.ts, and the ones before before it.
	// TypeScript 7's `isDeclarationContainedBy` compares the ranges without their files, so it takes those two for
	// `Item`'s own members: `Item` then lists them first (the own ones sorted by file and position).
	body: string;
	icon: string;
	rank: number;
}
