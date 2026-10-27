// for-of statements: one loop builder per iterable kind, and $range

declare function variadicIterator(): IterableFunction<LuaTuple<[number, ...Array<string>]>>;
declare function labeledIterator(): IterableFunction<LuaTuple<[key: string, value: number, end: boolean]>>;
declare function plainIterator(): IterableFunction<number>;
declare function getArray(): Array<string>;

export function arrays(values: Array<number>, tuple: [string, number], readonlyValues: ReadonlyArray<boolean>) {
	for (const v of values) {
		print(v);
	}
	for (const v of tuple) {
		print(v);
	}
	for (const v of readonlyValues) {
		if (v) continue;
		print(v);
	}
	for (const v of getArray()) {
		print(v);
	}
	for (const v of [1, 2, 3]) {
		print(v);
	}
	let last: number | undefined;
	for (last of values) {
		print(last);
	}
	const holder = { value: 0 };
	for (holder.value of values) {
		print(holder.value);
	}
	return last;
}

export function collections(set: Set<string>, map: Map<string, number>, shared: SharedTable, weak: WeakMap<object, string>) {
	for (const v of set) {
		print(v);
	}
	for (const [k, v] of map) {
		print(k, v);
	}
	for (const [, v] of map) {
		print(v);
	}
	for (const [k] of map) {
		print(k);
	}
	for (const [k, v = 5] of map) {
		print(k, v);
	}
	for (const entry of map) {
		print(entry[0], entry[1]);
	}
	for (const [k, v] of shared) {
		print(k, v);
	}
	for (const [k, v] of weak) {
		print(k, v);
	}
	let key = "";
	let value = 0;
	for ([key, value] of map) {
		print(key, value);
	}
	for ([key, value = 1] of map) {
		print(key, value);
	}
	let pair: [string, number];
	for (pair of map) {
		print(pair[0]);
	}
	return [key, value];
}

export function strings(text: string) {
	for (const char of text) {
		print(char);
	}
	for (const char of "abc") {
		print(char);
	}
}

export function iterableFunctions(instance: Instance, values: Map<string, number>) {
	for (const [k, v] of pairs(values)) {
		print(k, v);
	}
	for (const pair of pairs(values)) {
		print(pair[0], pair[1]);
	}
	for (const [i, child] of ipairs(instance.GetChildren())) {
		print(i, child);
	}
	for (const entry of labeledIterator()) {
		print(entry[0], entry[1], entry[2]);
	}
	for (const tuple of variadicIterator()) {
		print(tuple[0]);
	}
	let assigned: LuaTuple<[number, ...Array<string>]>;
	for (assigned of variadicIterator()) {
		print(assigned[0]);
	}
	for (const n of plainIterator()) {
		print(n);
	}
	for (const word of ("a b").gmatch("%a+")) {
		print(word);
	}
}

function* generator() {
	yield 1;
	yield 2;
}

export function generators() {
	for (const n of generator()) {
		print(n);
	}
	let last = 0;
	for (last of generator()) {
		print(last);
	}
	return last;
}

export function ranges(n: number, step?: number) {
	for (const i of $range(1, 10)) {
		print(i);
	}
	for (const i of $range(10, 1, -1)) {
		print(i);
	}
	for (const i of $range(0, n, 2)) {
		print(i);
	}
	for (const i of $range(0, n, step as number)) {
		print(i);
	}
	for (const i of $range(0, n, -(step as number))) {
		print(i);
	}
	let i = 0;
	for (i of $range(1, n)) {
		print(i);
	}
	return i;
}

export function tryInLoops(values: Array<number>) {
	for (const v of values) {
		try {
			if (v === 0) continue;
			if (v === 1) break;
			print(v);
		} catch (e) {
			print(e);
		}
	}
	for (let i = 0; i < values.size(); i++) {
		try {
			if (i === 2) continue;
			print(() => i);
		} finally {
			print("done");
		}
	}
}
