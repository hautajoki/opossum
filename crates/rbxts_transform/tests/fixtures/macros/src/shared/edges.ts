// macros in optional chains, chained macros, macro values as operands, element access calls

export function optional(maybeArr: Array<number> | undefined, maybeMap?: Map<string, number>) {
	maybeArr?.push(1);
	const size = maybeArr?.size();
	const value = maybeMap?.get("a");
	maybeMap?.set("b", 2);
	print(size, value);
}

export function chained(set: Set<string>, map: Map<string, number>, arr: Array<number>) {
	const both = set.add("a").add("b");
	map.set("a", 1).set("b", 2);
	const sorted = arr.sort().map((v) => v + 1);
	print(both, sorted, arr.filter((v) => v > 0).size());
}

export function operands(arr: Array<number>, getArr: () => Array<number>, f: () => number, t: keyof CheckableTypes) {
	getArr().push(f());
	arr.push(arr.size());
	arr.insert(arr.size(), f());
	if (arr.includes(f())) {
		print(typeIs(f(), t));
	}
	print(arr["includes"](f()), arr["indexOf"](1));
	const n = new Array<number>(f());
	return n.size() + getArr().size();
}
