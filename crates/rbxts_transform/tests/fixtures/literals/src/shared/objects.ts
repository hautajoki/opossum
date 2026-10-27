// Hand-written literals fixture: object literals.

declare function getKey(): string;
declare function getRecord(): { x: number };
declare const maybe: { a: number } | undefined;
declare const record: { x: number; y: string };
declare let n: number | undefined;

const key = "k";
const a = 1;

// function declarations first: TypeScript 5 lists them first among the exports, TypeScript 7 in source order
export function build(x: number) {
	return { x, y: x + 1, nested: { x } };
}

export function passes() {
	print({ a: 1 }, { ...record });
}

export const plain = { a: 1, b: "two", "c-d": true, 3: 4, [key]: 5, end: undefined };
export const shorthand = { a, key };
export const computed = { [getKey()]: 1, [`t${a}`]: 2, ["literal"]: 3 };
export const empty = {};
export const keys = { [-1]: "negative", 1.5: "fraction", [`plain`]: 0, ["valid"]: 1, ["not valid"]: 2 };
export const nested = { inner: { deep: { deeper: a } }, list: [1, 2] };

// statements needed by a value end the inline map
export const withPrereqs = { first: getKey(), second: n ?? 2, third: 3 };
// and so do those needed by a computed key
export const keyPrereqs = { first: 1, [n ?? "fallback"]: 2, last: 3 };

// spread first: table.clone
export const spreadFirst = { ...record, z: 1 };
// spread after fields: a loop
export const spreadAfter = { z: 1, ...record };
// a spread that may be undefined
export const spreadMaybe = { ...maybe, b: 2 };
export const spreadMaybeAfter = { b: 2, ...maybe };
// spread of an object literal and of a call
export const spreadLiteral = { ...{ q: 1 }, r: 2 };
export const spreadCall = { ...getRecord() };
export const twoSpreads = { ...record, ...maybe, last: true };

// a computed method name with statements: they go before the object, which stays inline
export const methodKeyPrereqs = {
	first: 1,
	[n ?? "fallback"]() {
		return 2;
	},
	last: 3,
};
