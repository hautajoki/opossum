// `super.m()` and `super.m?.()` claimed by symbol, and a `super` callee whose key `property_key` rewrites: each keeps
// `self` as the receiver.
export class Sub extends SeamBase {
	constructor(value: number) {
		super(value);
	}
	m(v: number) {
		return super.m(v);
	}
	n(v: number) {
		return super.n?.(v) ?? -1;
	}
	probeKeyed() {
		return super.probeKeyed() + 1;
	}
}
