// Hand-written T2 exprs fixture: delete, await, tagged templates, import().

interface Bag {
	x?: number;
	[k: string]: number | undefined;
}

declare function bagKey(): string;
declare function tag(strings: Array<string>, ...values: Array<unknown>): string;
declare const tags: { upper: (strings: Array<string>, ...values: Array<unknown>) => string };
declare function getTag(): typeof tag;

export function deletes(bag: Bag) {
	delete bag.x;
	delete bag[bagKey()];
	const removed = delete bag.y;
	return removed;
}

export async function awaits(p: Promise<number>) {
	const a = await p;
	const b = await (p as Promise<number>);
	await p;
	return a + b;
}

export function tagged(a: number, b: string) {
	const plain = tag`plain text`;
	const withValues = tag`a ${a} b ${b} end`;
	const escapes = tag`line\nnext A \`tick\``;
	const member = tags.upper`x${a}`;
	const called = getTag()`y${b}z`;
	let i = 0;
	const ordered = tag`${i} ${i++} ${i}`;
	return plain + withValues + escapes + member + called + ordered;
}

export async function dynamicImport() {
	const promise = import("shared/conditional");
	const mod = await import("./generators");
	print(promise, mod);
}
