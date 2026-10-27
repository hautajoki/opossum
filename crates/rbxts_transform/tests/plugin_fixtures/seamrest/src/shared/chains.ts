// Accesses and calls inside chains, offered to the expression hooks (probe-exprcall): before anything of the chain is
// compiled (before the first `?.`), or inside the nil check on their compiled base.
export function lazyLink() {
	return getA2().probeAccB?.c;
}
export function eagerLink() {
	return getA()?.probeAccB.c;
}
export function takenLink() {
	return getA()?.probeAccTakeB.c;
}
export function elementLink() {
	return getA()?.["probeAccB"].c;
}
export function deepLink() {
	return getA()?.plain?.probeAccB.c;
}
export function callInChain() {
	return getExprs()?.probeExprM(next()).value;
}
