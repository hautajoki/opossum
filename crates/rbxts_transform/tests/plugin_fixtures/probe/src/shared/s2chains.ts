// the expression hook (probe-exprcall) sees calls inside chains: before anything of the chain is compiled, or,
// after a `?.` link, inside the nil check on its compiled base
export const c1 = probeExprChain(1).value;
export const c2 = maybeExprs?.probeExprM(2).value;
// assignments the hook lowers itself: as a statement, or as a value
probeTarget.x = 1;
probeTarget.x++;
export const c3 = (probeTarget.x = 2);
// asked once: declined by the expression hook, then the call hook compiles the call itself
export const c4 = probeAskOnce(4);
