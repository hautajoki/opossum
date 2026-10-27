// Inheritance chains. TypeScript 5 lists a type's own members, then each base's list in turn (skipping the names it
// already has); TypeScript 7 lists its own members, then all inherited ones sorted by declaration position.

export interface Root {
	kind: string;
	size: number;
}

export interface Middle extends Root {
	size: number;
	label: string;
}

/** Redeclares a member of `Middle`. TS5: size, label, kind. TS7: size, kind, label. */
export interface Leaf extends Middle {
	size: number;
}

/** No members of its own. TS5: size, label, kind. TS7: kind, size, label. */
export interface Alias extends Middle {}

export interface First {
	a: number;
}

export interface Second {
	b: number;
}

/** Bases in the reverse of their declaration order. TS5: b, a. TS7: a, b. */
export interface Both extends Second, First {}

export interface Boxed<T> extends Middle {
	value: T;
}

/** Through a generic base. TS5: size, value, label, kind. */
export interface BoxedLeaf extends Boxed<number> {
	size: number;
}

/** An intersection lists each part's properties in turn. */
export type Mixed = Leaf & Both;

export class BaseClass {
	x = 1;
	constructor(public y: number) {}
	z = 2;
}

export class DerivedClass extends BaseClass {
	w = 3;
}

/** TS5: w, x, y, z. TS7: x, y, z, w. */
export class LeafClass extends DerivedClass {}

export declare const leafValue: Leaf;
export declare const boxedValue: Boxed<string>;
export declare const mixedValue: Mixed;

export const computedKey = "dyn";

/** A late-bound (computed) member: TS5 lists it after the bound ones. TS5: tail, dyn, kind, size. TS7: dyn, tail, kind, size. */
export interface WithComputed extends Root {
	[computedKey]: number;
	tail: string;
}
