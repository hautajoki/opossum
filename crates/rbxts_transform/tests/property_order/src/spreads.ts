// Spreads TypeScript 5 orders by its spread rules: unspread class members, union operands, generic operands.

const pair = { a1: 1, a2: 2 };

class Instance {
	m() {
		return 1;
	}
	f = 1;
	get g() {
		return 2;
	}
	private p = 3;
	z = 4;
}
const instance = new Instance();

/** Methods, accessors and private members of a class are not spread. TS5: f, z, m, g, q. */
const withInstance = { ...{ m: 1, g: 2, p: 5, q: 0 }, ...instance };
export type WithInstance = typeof withInstance;

declare const flag: boolean;
declare const count: number;

/** A falsy part of a union operand spreads nothing. TS5: k0, k3, k2, k1. */
const andSpread = { k1: 1, ...(flag && { k3: 3, k2: 2 }), k0: 0 };
export type AndSpread = typeof andSpread;

/** TS5: t0, t3, t2, t1. */
const emptySpread = { t1: 1, ...(flag ? { t3: 3, t2: 2 } : {}), t0: 0 };
export type EmptySpread = typeof emptySpread;

/** TS5: n0, n3, n2, n1. */
const countSpread = { n1: 1, ...(count && { n3: 3, n2: 2 }), n0: 0 };
export type CountSpread = typeof countSpread;

interface UFirst {
	u2: number;
	u1: number;
}
interface USecond {
	u3: number;
	u2: number;
}
declare const uFirst: UFirst;
declare const uSecond: USecond;

/** A union of objects spreads each member: a union of literal types. */
const unionSpread = { q: 1, ...(flag ? uFirst : uSecond), r: 2 };
/** TS5: r, u2, u1, q. */
export type UnionFirst = Extract<typeof unionSpread, { u1: number }>;
/** TS5: r, u3, u2, q. */
export type UnionSecond = Extract<typeof unionSpread, { u3: number }>;

interface GX {
	gx2: number;
	gx1: number;
}

/** A generic operand makes an intersection; the parts around it are spread separately. */
function make<T>(t: T) {
	return { gz: 1, ...pair, gy: 2, ...t, gx: 3 };
}
/** TS5: gy, a1, a2, gz, gx2, gx1, gx. */
export type Made = ReturnType<typeof make<GX>>;

interface Source {
	s2: number;
	s1: number;
}
interface SourceD extends Source {
	sd: number;
	s1: number;
}

/** A filtering `as` clause keeps the source order. TS5: sd, s1, s2. */
export type Filtered = { [K in keyof SourceD as K]: SourceD[K] };

export declare const withInstanceValue: WithInstance;
export declare const andSpreadValue: AndSpread;
export declare const emptySpreadValue: EmptySpread;
export declare const countSpreadValue: CountSpread;
export declare const unionFirstValue: UnionFirst;
export declare const unionSecondValue: UnionSecond;
export declare const madeValue: Made;
export declare const filteredValue: Filtered;
