// Hand-written T2 exprs fixture: mixed edge cases.

declare function tag(strings: Array<string>, ...values: Array<unknown>): string;
declare function pick(): typeof tag;
declare function n(): number;

interface Bag {
	x?: number;
	inner: Bag;
}

// only one branch has prerequisites
export function oneSided(flag: boolean, bag: Bag) {
	let i = 0;
	const a = flag ? 1 : i++;
	const b = flag ? bag.inner.x ?? 2 : 3;
	return a + b + i;
}

// statement conditional whose condition needs prerequisites and truthiness checks
export function statementCondition(bag: Bag) {
	let i = 0;
	(bag.x ?? i++) ? print("a") : print("b");
	n() ? print("c") : undefined;
}

// delete as a value
export function deleteValue(bag: Bag) {
	print(delete bag.inner.x, delete bag.x);
}

// a tag with prerequisites, non-ASCII and long-bracket text
export function tagEdges(bag: Bag) {
	let i = 0;
	const r = (i++ > 0 ? pick() : tag)`héllo ${i} 😀 ]] end`;
	const s = tag`a\b ${"x"}${bag.x}`;
	const t = tag`back\\slash "quoted" ${s}\t'single'`;
	return r + s + t;
}

// logical assignments inside arguments
export function argOrder(bag: Bag, s: string) {
	let i = 0;
	print(i++, (bag.x ??= i), (s ||= "q"), i);
}

// yield inside other expressions
export function* yields(flag: boolean) {
	const v: number = flag ? yield 1 : 2;
	print((yield v) as number);
	const w: number = yield* inner();
	return w;
}

function* inner() {
	yield 1;
	return 2;
}
