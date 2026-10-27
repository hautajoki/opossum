// ReadonlyArray and Array macros, used as values and as statements

export function readonlyArrays(
	arr: Array<number>,
	strs: ReadonlyArray<string>,
	parts: Array<Instance>,
	maybeSep?: string,
) {
	print(arr.size(), arr.isEmpty(), strs.isEmpty());
	print(strs.join(), strs.join(", "), parts.join("-"), strs.join(maybeSep), strs.join(undefined));
	print(arr.includes(1), arr.includes(2, 3), arr.indexOf(4), arr.indexOf(5, 1));
	print(
		arr.every((v) => v > 0),
		arr.some((v, i) => v === i),
	);
	arr.forEach((v, i, a) => print(v, i, a));
	const each = arr.forEach((v) => print(v));
	const doubled = arr.map((v) => v * 2);
	const parents = parts.mapFiltered((p) => p.Parent);
	const defined = arr.filterUndefined();
	const big = arr.filter((v) => v > 10);
	const sum = arr.reduce((acc, v) => acc + v);
	const total = arr.reduce((acc, v, i) => acc + v * i, 0);
	const found = arr.find((v) => v === 3);
	const foundIndex = arr.findIndex((v) => v === 3);
	print(each, doubled, parents, defined, big, sum, total, found, foundIndex);
	arr.move(0, 1, 2);
	return arr.move(0, 1, 2, arr);
}

export function arrayMutators(arr: Array<number>, getArr: () => Array<number>, f: () => number) {
	arr.push();
	arr.push(1);
	arr.push(f(), 2, f());
	const len = arr.push(3);
	getArr().push(4);
	print(arr.pop());
	arr.pop();
	print(arr.shift());
	arr.shift();
	arr.unshift(5, 6);
	const len2 = arr.unshift(7);
	arr.insert(0, 8);
	arr.insert(f(), 9);
	print(arr.remove(0), arr.remove(f()));
	arr.unorderedRemove(1);
	const removed = arr.unorderedRemove(f());
	arr.sort();
	arr.sort((a, b) => a < b);
	const sorted = arr.sort();
	arr.clear();
	print(len, len2, removed, sorted, arr.clear());
	arr["push"](10);
	print(arr["size"](), getArr()["pop"]());
}

export function receivers(getArr: () => Array<number>, f: () => number) {
	print(getArr().size(), getArr().includes(f()));
	getArr().forEach((v) => print(v));
	const mapped = getArr().map((v) => v + f());
	getArr().unorderedRemove(f());
	return mapped;
}
