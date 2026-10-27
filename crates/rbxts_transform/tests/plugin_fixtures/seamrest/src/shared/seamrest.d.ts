// The ambient values of the seamrest fixture. `runtime/check.luau` provides each one, counting calls where a check
// needs to know how often something ran. Claims are the `probe` and `probe-exprcall` test plugins'
// (`rbxts_test_plugins`): `@probe <op>` names the call hook's operation.

/** Counts its calls; returns the count. */
declare function next(): number;

// --- optional calls ---

/** Claimed by receiver: `compile_receiver`, then `receiver:probed(args)`. */
interface Marked {
	readonly __probeMarker: true;
	probeMethod(value: number): number;
	probed(value: number): number;
}
/** A marked receiver whose method may be missing. */
interface MaybeMethod {
	readonly __probeMarker: true;
	probeMethod?(value: number): number;
	probed(value: number): number;
}
declare function getMarked(): Marked | undefined;
declare function getMaybeMethod(): MaybeMethod;
declare function getMaybeMaybeMethod(): MaybeMethod | undefined;

/** Non-method calls claimed by symbol. */
interface Holder {
	/** `compile_call(Function(useRecv), [Luau(compile_receiver), Original(args)…])` */
	/** @probe recvarg */
	fn?: (a: number) => number;
	/** `wrap(compile_expr(call))` */
	/** @probe self */
	sf?: (a: number) => number;
}
declare function getHolder(): Holder | undefined;
declare function useRecv(...args: Array<unknown>): number;

// --- super ---

/** A base class whose methods are claimed by symbol, and whose `__probeKey` marker rewrites `super.probeKeyed`. */
declare class SeamBase {
	readonly __probeKey: true;
	value: number;
	constructor(value: number);
	/** `wrap(compile_expr(call))` */
	/** @probe self */
	m(v: number): number;
	/** `compile_call(Original, [Original(v), Luau("injected"), Nil])` */
	/** @probe inject */
	n?(v: number, extra?: string, more?: unknown): number;
	probeKeyed(): number;
}

// --- chain links (probe-exprcall: accesses named `probeAcc…`, calls with `.probeExpr` in their callee) ---

interface AccA {
	probeAccB: { c: number };
	probeAccTakeB: { c: number };
	plain: AccA | undefined;
}
declare function getA(): AccA | undefined;
declare function getA2(): AccA;
interface ExprChain {
	probeExprM(a: number): { value: number };
}
declare function getExprs(): ExprChain | undefined;

// --- asked once ---

/** `wrap(compile_expr(call))`; probe-exprcall declines these first, warning each time it is asked. */
/** @probe self */
declare function probeAskOnce(a: number): number;
interface AskOnce {
	/** @probe self */
	probeAskOnceM(a: number): number;
}
declare function getAskOnce(): AskOnce | undefined;
/** @probe self */
declare class probeAskOnceNew {
	constructor(a: number);
}

// --- statements ---

declare const probeElseIf: boolean;

// --- JSX tags ---

declare function createElement(tag: unknown, props: unknown, ...children: Array<unknown>): unknown;
declare namespace createElement {
	namespace JSX {
		type Element = unknown;
		interface IntrinsicElements {
			frame: {};
		}
	}
}
declare const tags: { probeAccTag: () => unknown; plainTag: () => unknown };
interface KeyedTags {
	readonly __probeKey: true;
	Comp: () => unknown;
}
declare const keyedTags: KeyedTags;

// --- rewritten computed keys ---

interface DropKeyed {
	readonly __probeKey: true;
	a: number;
	s: string;
}
declare const dropKeyed: DropKeyed;
declare function getDropKeyed(): DropKeyed;
declare function keyName(): string;
