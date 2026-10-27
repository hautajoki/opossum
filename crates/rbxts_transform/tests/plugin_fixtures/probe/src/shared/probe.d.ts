// The calls the probe plugin claims by symbol: `@probe <op>` in the JSDoc.

/** @probe hoist */
declare function probeHoist<T>(value: T): T;

/** @probe prereq */
declare function probePrereq<T>(value: T): T;

/** @probe order */
declare function probeOrder(...values: Array<unknown>): Array<unknown>;

/** @probe inject */
declare function probeInject(value: unknown, extra?: string, more?: unknown): void;

/** @probe swap */
declare function probeSwap(a: number, b: string): number;

/** @probe late */
declare function probeLate(): string;

/** @probe capture */
declare function probeCapture<T>(value: T): T;

/** @probe root */
declare function probeRoot(): void;

/** @probe decorator */
declare function probeDecorator(label?: string): (ctor: object) => void;

/** @probe new */
declare class ProbeWidget {
	constructor(size: number);
}

/** @probe boom */
declare function probeBoom(): void;

/** @probe fail */
declare function probeFail(): void;

/** @probe err */
declare function probeErr(): void;

/** Claimed by receiver: the type has the marker member. */
interface Marked {
	readonly __probeMarker: true;
	probeMethod(value: number): number;
	probed(value: number): number;
}

/** Not claimed: no marker. */
interface Unmarked {
	probeMethod(value: number): number;
}

declare const marked: Marked;
declare const maybeMarked: Marked | undefined;
declare const unmarked: Unmarked;

/** Claimed by callee marker. */
declare const callable: ((value: number) => number) & { readonly __probeCallee: true };

/** `void probeVoid` is claimed by the expression fallback. */
declare const probeVoid: number;

/** @probe facts {@link marked} */
declare function probeFacts(): string;

/** @probe self */
declare function probeSelf(a: number): number;

/** A chain of calls the probe claims with `compile_expr` of the call itself. */
interface Chain {
	/** @probe self */
	s(a: number): number;
	inner: Chain;
}
declare const maybeChain: Chain | undefined;
declare function getChain(): Chain;
declare function sideEffect(): number;

// Declined after compile_expr of the call, a hoist, or a prereq: each an internal error (used by the tests only).

/** @probe declineafter */
declare function probeDeclineAfter(a: number): number;

/** @probe hoistdecline */
declare function probeHoistDecline(): number;

/** @probe prereqdecline */
declare function probePrereqDecline(): number;

/** A marked base class, for a `super.probeMethod()` call (used by the tests only). */
declare class MarkedBase {
	readonly __probeMarker: true;
	probeMethod(value: number): number;
}

/** Compiles its argument, then declines: an internal error when the argument's own claim hoisted (tests only). */
/** @probe argdecline */
declare function probeArgDecline(a: unknown): unknown;

/** `void probeVoidHoist` is declined by the expression fallback after a hoist: an internal error (tests only). */
declare const probeVoidHoist: number;

/** Decorators lowered with `compile_expr(call.node)`. */
/** @probe decoself */
declare function probeDecoSelf(label?: string): (ctor: object) => void;
/** @probe decoself */
declare function probeDecoSelfPlain(ctor: object): void;

/** Declare a local named `_hoisted` / `impl` when the compiler approves the name, then compile the argument. */
/** @probe approve */
declare function probeApprove<T>(v: T): T;
/** @probe approveimpl */
declare function probeApproveImpl<T>(v: T): T;

/** Compiles its receiver inside a capture, drops the statements and declines (tests only). */
interface CapChain {
	/** @probe caprecv */
	cr(a: number): number;
}
declare const capMap: Record<string, CapChain>;

/** `void void probeVoidSelf`: the outer hook compiles its own node and declines (tests only). */
declare const probeVoidSelf: number;

/** The claimed node through compile_in_order (a `new`, an eager and a lazy chain call, an expression hook). */
/** @probe inorderself */
declare class ProbeInOrder {
	constructor(a: number);
}
/** @probe inorderself */
declare function probeInOrderFn(a: number): number;
interface InOrderChain {
	/** @probe inorderself */
	io(a: number): number;
}
declare const maybeInOrder: InOrderChain | undefined;
declare function getInOrder(): InOrderChain;
declare const probeVoidInOrder: number;

/** A late string or mark_import_used, then declined: each an internal error (tests only). */
/** @probe latedecline */
declare function probeLateDecline(): string;
/** @probe markdecline */
declare function probeMarkDecline(a: unknown): unknown;

/** An expression-kind hook (probe-exprcall) wraps these calls; the probe's call hook still claims them inside. */
/** @probe self */
declare function probeExprSelf(a: number): number;
/** @probe inorderself */
declare function probeExprInOrder(a: number): number;
/** @probe new */
declare class ProbeExprWidget {
	constructor(size: number);
}

/** The claimed call compiled twice (tests only). */
/** @probe twice */
declare function probeTwice(a: number): number;

/** `probeStmtSelf;`: a statement hook compiles its own statement (tests only). */
declare const probeStmtSelf: number;

// --- S2 ---

/** Optional and non-method calls claimed by symbol: `compile_call(Function(useRecv), [receiver, args…])`. */
interface Holder {
	/** @probe recvarg */
	fn?: (a: number) => number;
	/** @probe self */
	sf?: (a: number) => number;
	/** @probe recvarg */
	mm?(a: number): number;
	inner: Holder;
}
declare const holder: Holder;
declare const maybeHolder: Holder | undefined;
declare function getHolder(): Holder;
declare function useRecv(...args: Array<unknown>): number;

/** `super.m()` calls claimed by symbol. */
declare class SelfBase {
	/** @probe self */
	m(v: number): number;
	/** @probe inject */
	n?(v: number, extra?: string, more?: unknown): number;
	/** @probe recvsuper */
	r(v: number): number;
}

/** Accesses whose receiver has `__probeKey`: `property_key` answers `k_<key>` (`key with space` for `swap`, the key
 * itself for `same`). */
interface Keyed {
	readonly __probeKey: true;
	a: number;
	b: number;
	s: string;
	opt?: number;
	swap: number;
	same: number;
	m(x: number): number;
	f: (x: number) => number;
	inner: Keyed;
}
declare const keyed: Keyed;
declare const maybeKeyed: Keyed | undefined;
declare function getKeyed(): Keyed;

/** Object literals compiled with edits. */
/** @probe objedit */
declare function probeObject<T>(value: T): T;
/** @probe objorder */
declare function probeObjectOrder<T>(value: T): T;
declare const extra: { e: number };

/** `compile_binary(PlusToken, a, b)`. */
/** @probe binary */
declare function probeBinary(a: unknown, b: unknown): unknown;

/** The locals so far: `"f<function> m<main chunk>"`. */
/** @probe locals */
declare function probeLocals(): string;

/** `hoist_spillable(compile_expr(a), "spilled")`. */
/** @probe spill */
declare function probeSpill<T>(a: T): T;

/** Misuse (tests only): `compile_call` twice; `compile_expr` then `compile_call`; an ancestor compiled;
 * `compile_binary` with an assignment operator. */
/** @probe calltwice */
declare function probeCallTwice(a: number): number;
/** @probe exprthencall */
declare function probeExprThenCall(a: number): number;
/** @probe ancestor */
declare function probeAncestor(): number;
/** @probe binbad */
declare function probeBinBad(a: number, b: number): number;

/** An expression hook (probe-exprcall) declines these; the call hook then compiles the call itself. */
/** @probe self */
declare function probeAskOnce(a: number): number;

/** Chain-inner calls for the expression hook (probe-exprcall). */
declare function probeExprChain(a: number): { value: number };
interface ExprChain {
	probeExprM(a: number): { value: number };
}
declare const maybeExprs: ExprChain | undefined;
/** Assignments the expression hook (probe-exprcall) lowers itself. */
declare const probeTarget: { x: number };
/** Misuse (tests only): `compile_expr` of the call's receiver, compiled already for a call after `?.`. */
interface ReceiverExpr {
	/** @probe recvexpr */
	re(a: number): number;
	probeExprBase(a: number): { value: number };
}
declare const maybeRe: ReceiverExpr | undefined;
declare const plainRe: ReceiverExpr;
/** Returns no value at all (tests only: rendering it panics). */
/** @probe none */
declare function probeNone(): number;
/** Prereqs with locals: a `do` block, a two-name `local` with a call, a local function of 201 locals. */
/** @probe doprereq */
declare function probeDoPrereq(): undefined;
/** @probe multiprereq */
declare function probeMultiPrereq(): void;
/** @probe fnprereq */
declare function probeFnPrereq(): void;
declare function mk(): object;
declare const nx: number | undefined;

// --- seam additions ---

/** Computed keys on a `__probeKey` receiver. */
declare function keyName(): string;
declare const anyKey: string;
interface KeyedList {
	readonly __probeKey: true;
	[index: number]: number;
}
declare const keyedList: KeyedList;
/** Optional chains over `__probeKey` receivers. */
interface KeyedMore extends Keyed {
	g(x: number): KeyedMore;
	h?: (x: number) => KeyedMore;
}
declare function getMaybeKeyed(): KeyedMore | undefined;
declare function val(): number;
/** Link texts: `{@link Name text}` and `{@link Name | text}`. */
/** @probe links {@link marked} {@link marked the marked one} {@link maybeMarked | piped text } {@link keyed |} */
declare function probeLinks(): string;
declare const probeTakeComputed: number;

/** `runtime_member("Promise", "is")`. */
/** @probe runtime */
declare function probeRuntime(): unknown;
/** `synthesized_string` of tricky texts. */
/** @probe synth */
declare function probeSynth(): Array<string>;
/** The same warning twice: one is kept. */
/** @probe diagtwice */
declare function probeDiagTwice(): void;
/** `insert_before_statement([comment, print])`, then the argument. */
/** @probe before */
declare function probeBefore<T>(a: T): T;
/** `hoist_spillable_named(a, "constant_22")`. */
/** @probe named */
declare function probeNamed<T>(a: T): T;
/** The file's identifier texts starting with `jd` or `lit`. */
/** @probe idents */
declare function probeIdents(): string;
/** The argument type's properties in TypeScript 5's order. */
/** @probe props */
declare function probeProps(a: unknown): string;
/** The argument, then `import_named("./runtime", "other")`. */
/** @probe importafter */
declare function probeImportAfter<T>(a: T): [T, number];
/** `import_named(path, export)` of its two string literal arguments. */
/** @probe importfrom */
declare function probeImportFrom(path: string, name: string): unknown;
/** A warning at the call with the text of `text`, then the value. */
/** @probe diagsame */
declare function probeDiagSame<T>(value: T, text: string, asMessage?: boolean): T;
/** A warning at this declaration, with related information when given an argument. */
/** @probe diagdecl */
declare function probeDiagDecl(related?: boolean): undefined;
