// compile_object_literal: `a` renamed, `b` replaced (its value not evaluated), `e` appended; spreads and computed
// keys kept
const key = "k";
export const o1 = probeObject({ a: 1, b: sideEffect(), ...extra, "c d": 2, [key]: 3 });
// emitted in another order, evaluated in source order
export const o2 = probeObjectOrder({ x: sideEffect(), y: 2, z: sideEffect() + 1 });
export const b1 = probeBinary("x", 1);
export const b2 = probeBinary(1, sideEffect());
