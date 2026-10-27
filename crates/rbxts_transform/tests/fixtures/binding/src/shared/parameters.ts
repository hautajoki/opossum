export function objectParameter({ a, b = 2 }: { a: number; b?: number }) {
	return a + b;
}

export function arrayParameters([x, y]: [number, number], [z = 3]: Array<number> = []) {
	return x + y + z;
}

export function restTuple(...[p, q]: [number, string]) {
	return `${p}${q}`;
}

export function restTupleNested(...[, q = "q", [r], { s }]: [number, string | undefined, Array<number>, { s: boolean }]) {
	return [q, r, s];
}

export function restWithSpread(...[p, ...more]: Array<number>) {
	return p + more.size();
}

export function nestedParameter({ inner: { deep }, list: [first, ...others] }: { inner: { deep: string }; list: Array<string> }) {
	return deep + first + others.size();
}

export function objectRestParameter({ a, ...rest }: { a: number; b: number; c: string }) {
	return [a, rest];
}

export function defaultedPattern({ a }: { a: number } = { a: 1 }, [b]: [number] = [2]) {
	return a + b;
}

export const arrow = ({ a }: { a: number }, [b]: [number]) => a + b;

export const method = {
	run({ value }: { value: string }) {
		return value;
	},
};
