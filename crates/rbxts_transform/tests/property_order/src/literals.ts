// Type literals, object literals with spreads and homomorphic mapped types.

import type { Both, Leaf } from "./chain";

export declare const tag: unique symbol;
export const lateKey = "late";

interface Head {
	head: number;
	shared: number;
}

/** A type literal lists its computed members after the others. TS5: z, y, x, @tag, late. */
export type Literal = { z: number; [tag]: string; y: number; [lateKey]: number; x: number };

/** An intersection with a type literal part. TS5: head, shared, z, y, x, @tag, late. */
export type HeadLiteral = Head & { z: number; [tag]: string; y: number; [lateKey]: number; x: number };

/** A generic type literal, instantiated. TS5: value, first, late. */
export type Generic<T> = { value: T; [lateKey]: T; first: T };

const baseObject = { sa: 1, sb: 2 };
const spreadObject = { sc: 3, ...baseObject, sd: 4, sa: 5 };
const plainObject = { pz: 1, py: 2, px: 3 };

/** A spread lists the right side, then what the left side adds. TS5: sd, sa, sb, sc. */
export type Spread = typeof spreadObject;

/** A base that is a spread type. TS5: own, sd, sa, sb, sc. */
export interface FromSpread extends Spread {
	own: number;
}

/** TS5: pz, py, px. */
export type Plain = typeof plainObject;

type MyPartial<T> = { [P in keyof T]?: T[P] };

/** A homomorphic mapped type follows its source. TS5: size, label, kind. */
export type PartialLeaf = MyPartial<Leaf>;

/** TS5: b, a. */
export type PartialBoth = Partial<Both>;

export declare const literalValue: Literal;
export declare const headLiteralValue: HeadLiteral;
export declare const genericValue: Generic<number>;
export declare const spreadValue: Spread;
export declare const plainValue: Plain;
export declare const partialLeafValue: PartialLeaf;
export declare const partialBothValue: PartialBoth;
