// optional method calls by receiver: the receiver is `_self`, the call lowered inside the nil check
export const o1 = marked.probeMethod?.(1);
export const o2 = maybeMarked?.probeMethod?.(2);
marked.probeMethod?.(3);
// non-method optional calls by symbol: compile_receiver binds the receiver before the callee is read from it
export const f1 = holder.fn?.(1);
export const f2 = getHolder().inner.fn?.(sideEffect());
export const f3 = maybeHolder?.fn?.(3);
export const f4 = holder.mm?.(4);
export const f5 = holder.sf?.(5);
export const f6 = holder["fn"]?.(6);
// super calls by symbol
export class Sub extends SelfBase {
	m(v: number) {
		return super.m(v);
	}
	n(v: number) {
		return super.n?.(v) ?? 0;
	}
}
