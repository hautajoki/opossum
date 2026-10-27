/**
 * A header comment with a blank line after it: TypeScript's printer keeps it above the plugin imports.
 */

// the first statement's own comment stays with it
const impl = 1;
const constant_22 = 2;

/**
 * @param jdOnlyDoc a JSDoc parameter name, and a link: {@link jdLinked}
 * @jdTag
 */
export function described(n: number) {
	return n + impl + constant_22;
}
const literals = { litKey: 1, "litQuoted": 2 };
export const e1 = literals["litIndex" as "litKey"];
export const e2 = literals["litQuoted"];

// `impl` and `constant_22` are taken: `impl_1`, `constant_22_1`
export const swapped = probeSwap(1, "a");
export const named = probeNamed([1]);
export const runtime = probeRuntime();
export const synth = probeSynth();
export const idents = probeIdents();
// with plugins, a literal with separators is its value (as TypeScript's printer reprints it)
export const big = 1_000_000;
export const hex = 0xff_ff;
export const plain = 0xff;
export const nums = [1_000.5, 1e1_0, 0b1010_1010, 0o7_7, 123_456_789_012_345_678_901, 0.000_001];
probeDiagTwice();

interface PA {
	x: number;
	y: number;
}
interface PB extends PA {
	y: number;
	w: number;
}
interface PC extends PB {
	c: number;
}
declare const pc: PC;
export const props = probeProps(pc);
