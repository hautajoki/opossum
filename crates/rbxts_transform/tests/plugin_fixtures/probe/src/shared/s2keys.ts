// property_key: every access whose receiver has `__probeKey`, in every position
export const r1 = keyed.a;
export const r2 = keyed["b"];
keyed.a = 1;
keyed.b += 2;
keyed.s += "x";
keyed.a++;
export const r3 = keyed.a++;
export const r4 = ++keyed.b;
delete keyed.opt;
export const r5 = keyed.m(1);
export const r6 = keyed.f(2);
export const r7 = maybeKeyed?.a;
export const r8 = maybeKeyed?.m(3);
export const r9 = keyed.m?.(4);
[keyed.a, keyed.b] = [1, sideEffect()];
({ a: keyed.a } = { a: 5 });
export const r10 = keyed.swap;
keyed.swap = 3;
export const r11 = keyed.swap++;
export const r12 = getKeyed().inner.inner.a;
for (keyed.a of [1, 2]) {
	print(keyed.a);
}
keyed.a ??= 4;
export const r13 = keyed.same;
export const r14 = keyed["swap"];
