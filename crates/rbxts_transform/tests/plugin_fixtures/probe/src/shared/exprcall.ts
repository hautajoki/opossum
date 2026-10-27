// an expression-kind hook compiles its own call (or `new`); the call plugins still claim it, inside
export const a = probeExprSelf(sideEffect());
export const b = probeExprInOrder(1);
export const c = new ProbeExprWidget(5);
