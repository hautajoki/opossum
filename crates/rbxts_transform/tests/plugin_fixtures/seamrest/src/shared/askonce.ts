// Declined by the expression hook (probe-exprcall), then claimed by the call hook, which compiles the node itself:
// the expression hook is asked once per node.
export function plain() {
	return probeAskOnce(next());
}
export function inChain() {
	return getAskOnce()?.probeAskOnceM(next());
}
export function construct() {
	return new probeAskOnceNew(next());
}
