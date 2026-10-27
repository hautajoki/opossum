export function strings(text: string, index: number) {
	const { 0: c0, [index]: cAt, [1]: c1 } = text;
	const { "2": c2 } = "abc";
	return [c0, cAt, c1, c2];
}

export function arrays(list: Array<number>, index: number) {
	const { 0: first, [index]: atIndex, [index + 1]: following } = list;
	const { 1: second, ...others } = list;
	return [first, atIndex, following, second, others];
}

export function maps(map: Map<string, number>, readonly: ReadonlyMap<string, number>) {
	const [a] = map;
	const [b, c] = readonly;
	let d: [string, number] | undefined;
	[d] = readonly;
	return [a, b, c, d];
}

export function tuples(tuple: [number, string, ...Array<boolean>]) {
	const [n, s, ...flags] = tuple;
	let x = 0;
	let rest: Array<string | boolean> = [];
	[x, ...rest] = tuple;
	let flag: boolean | undefined;
	[, , ...[flag]] = tuple;
	return [n, s, flags, x, rest, flag];
}

export function defaults(obj: { a?: number; b?: { c?: string } }) {
	const { a = 1, b: { c = "c" } = {} } = obj;
	let z = 0;
	let w = "";
	({ a: z = 3, b: { c: w = "w" } = {} } = obj);
	return [a, c, z, w];
}

export function sideEffects() {
	const order = new Array<number>();
	const log = (n: number) => {
		order.push(n);
		return n;
	};
	const target = [0, 0, 0];
	[target[log(0)], target[log(1)]] = [log(2), log(3)];
	const [p = log(4), q = log(5)] = [undefined, log(6)];
	return [order, p, q];
}

export class Holder {
	value = 1;
	constructor(private readonly items: Array<number>, { a, b }: { a: number; b: number }) {
		print(a, b);
	}
	method([first, ...rest]: Array<number>, { x = 2 }: { x?: number } = {}) {
		const { value } = this;
		[this.value] = this.items;
		({ value: this.value } = { value: first });
		return [first, rest, x, value];
	}
	static make(...[a, b]: [number, number]) {
		return new Holder([a, b], { a, b });
	}
}
