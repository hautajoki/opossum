// Holes in an optimized LuaTuple (or array literal) assignment. roblox-ts never declared the holes' temporaries, so
// `[, q, , p] = tup()` wrote the globals `_` and `_1`.
declare function tup(): LuaTuple<[number, number, number, number]>;

export function tupleHoles() {
	let q = 0;
	let p = 0;
	[, q, , p] = tup();
	return [q, p];
}

export function literalHoles() {
	let a = 0;
	[, a] = [tup()[0], 5];
	return a;
}
