// Generic and union spread operands, and a renaming `as` clause.

interface Left {
	ta: number;
	tb: number;
}
interface Right {
	tb: number;
	ta: number;
}
declare const left: Left;
declare const right: Right;

/** Two parts with the same names around a generic operand. TS5: ta, tb, tz. */
function tie<T extends object>(t: T) {
	return { ...left, ...t, ...right };
}
export type Tied = ReturnType<typeof tie<{ tz: number }>>;

interface Options {
	oa: number;
	oc: number;
}

/** An optional generic operand spreads as a partial of its constraint. TS5: ob, oa, oc, ox. */
function withOptions<T extends Options>(t: T | undefined) {
	return { ox: 1, ...t, ob: 2 };
}
export type WithOptions = ReturnType<typeof withOptions<Options>>;

interface RA {
	readonly rb: number;
	ra: number;
}
interface RC {
	readonly rd: number;
	rc: number;
}
interface RE extends RC, RA {
	readonly re: number;
}

/** A generic intersection operand: the next run is spread into its last part. TS5: tz, a, re, rd, rc, rb, ra. */
function intersected<T extends object>(t: T & RE) {
	return { ...t, a: 1 };
}
export type Intersected = ReturnType<typeof intersected<{ tz: number }>>;

interface PX<T> {
	x2: T;
	x1: T;
}
interface PY<T> {
	y2: T;
	y1: T;
}
interface PXY<T> extends PX<T>, PY<T> {}
interface PYX<T> extends PY<T>, PX<T> {}
declare const either: PXY<string> | PYX<number>;
const eitherSpread = { ...either };
/** Union members with the same names: told apart by their symbols. TS5: x2, x1, y2, y1. */
export type EitherString = Extract<typeof eitherSpread, { x1: string }>;
/** TS5: y2, y1, x2, x1. */
export type EitherNumber = Extract<typeof eitherSpread, { x1: number }>;

interface EvBase {
	onZ: number;
	y: number;
}
interface EvBase2 {
	onY: number;
	x: number;
}
interface Ev extends EvBase2, EvBase {
	onA: number;
	w: number;
}
type Getters<T> = { [K in keyof T as `get_${K & string}`]: T[K] };
/** A renaming `as` clause follows the source order. TS5: get_onA, get_w, get_onY, get_x, get_onZ, get_y. */
export type EvGetters = Getters<Ev>;

export declare const tiedValue: Tied;
export declare const withOptionsValue: WithOptions;
export declare const intersectedValue: Intersected;
export declare const eitherStringValue: EitherString;
export declare const eitherNumberValue: EitherNumber;
export declare const evGettersValue: EvGetters;
