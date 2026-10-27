// Optional method calls (`o?.m()`, `o.m?.()`, `o?.m?.()`) and non-method optional calls (`o?.f?.()`), claimed: the
// receiver is evaluated once, and nothing of the call (its arguments, the claimed value) runs when the chain stops.
export function receiverOptional() {
	return getMarked()?.probeMethod(next());
}
export function methodOptional() {
	return getMaybeMethod().probeMethod?.(next());
}
export function bothOptional() {
	return getMaybeMaybeMethod()?.probeMethod?.(next());
}
export function fnOptional() {
	return getHolder()?.fn?.(next());
}
export function selfOptional() {
	return getHolder()?.sf?.(next());
}
export function statementOptional() {
	getMarked()?.probeMethod(next());
}
