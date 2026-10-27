export function* values() {
	yield 1;
	yield 2;
	yield 3;
}

export function strings() {
	const str = "héllo";
	const [ch0, ch1, ...chRest] = str;
	const [, second] = str;
	let c = "";
	let cs: Array<string> = [];
	[c, ...cs] = str;
	return [ch0, ch1, chRest, second, c, cs];
}

export function collections() {
	const set = new Set([1, 2, 3]);
	const [s0, s1] = set;
	const [sOnly] = set;
	const [sFirst, , ...sRest] = set;
	const map = new Map([
		["a", 1],
		["b", 2],
	]);
	const [[k0, v0], entry1] = map;
	const [, mapSecond] = map;
	let assigned = 0;
	[assigned] = set;
	return [s0, s1, sOnly, sFirst, sRest, k0, v0, entry1, mapSecond, assigned];
}

export function iterators() {
	const [m0, m1] = "a b".gmatch("%a");
	const [g0, , ...gRest] = values();
	const source: IterableIterator<number> = values();
	const [i0, i1 = 7] = source;
	const record = { a: 1 };
	const [[pk, pv]] = pairs(record);
	const [ipFirst] = ipairs([1, 2]);
	let assigned = 0;
	let rest: Array<number> = [];
	[assigned, ...rest] = values();
	return [m0, m1, g0, gRest, i0, i1, pk, pv, ipFirst, assigned, rest];
}

export function sharedTables() {
	const sharedTable = new SharedTable();
	sharedTable.a = 1;
	const { a } = sharedTable;
	const [first, ...others] = sharedTable;
	return [a, first, others];
}

export function nestedRest() {
	let value = 0;
	let tail: Array<number> = [];
	[...[value, ...tail]] = new Set([1]);
	let key: string | number = "";
	[...[[key, value]]] = new Map([["key", 2]]);
	[...[value]] = values();
	[...[key]] = "🍓a";
	[...[[key]]] = string.gmatch("a,b", "[^,]+");
	return [value, tail, key];
}
