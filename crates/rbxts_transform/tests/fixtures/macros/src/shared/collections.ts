// ReadonlySet, Set, ReadonlyMap and Map macros, and the constructor macros

export function sets(set: Set<string>, ro: ReadonlySet<string>, key: string) {
	print(ro.isEmpty(), ro.size(), ro.has(key), set.has("a"));
	ro.forEach((v, v2, s) => print(v, v2, s));
	set.add(key);
	const same = set.add("x");
	set.delete(key);
	const existed = set.delete("y");
	set.clear();
	print(same, existed, set.size());
}

export function maps(map: Map<string, number>, ro: ReadonlyMap<string, number>, getKey: () => string) {
	print(ro.isEmpty(), ro.size(), ro.has("a"), ro.get("a"), ro.get(getKey()));
	ro.forEach((v, k, m) => print(v, k, m));
	map.set("a", 1);
	map.set(getKey(), map.size());
	const returned = map.set("b", 2);
	map.delete("a");
	const existed = map.delete(getKey());
	const value = map.get("b");
	if (value !== undefined) {
		print(value + 1);
	}
	map.clear();
	print(returned, existed, map["get"]("c"));
}

export function order(getMap: () => Map<string, number>, k: () => string, v: () => number) {
	getMap().set(k(), v());
	print(getMap().get(k()));
	const has = getMap().has(k());
	const removed = getMap().delete(k());
	return has && removed;
}

export function constructors(
	arr: Array<string>,
	entries: Array<[string, number]>,
	instances: Array<[Instance, number]>,
	n: number,
) {
	const a = new Array<number>();
	const b = new Array<number>(n);
	const c = new Set<string>();
	const d = new Set(arr);
	const e = new Map<string, number>();
	const f = new Map(entries);
	const g = new WeakSet<Instance>();
	const h = new WeakMap<Instance, number>();
	const i = new ReadonlySet(arr);
	const j = new ReadonlyMap(entries);
	const k = new WeakMap(instances.map((entry) => entry));
	print(a, b, c, d, e, f, g, h, i, j, k);
}

export function classes(n: number) {
	const part = new Instance("Part");
	const v = new Vector3(1, 2, n);
	const zero = new Vector3();
	const cf = new CFrame(v);
	print(part, v, zero, cf);
}
