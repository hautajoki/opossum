import { impl, other } from "./runtime";

export const a = impl("a", other);
// `impl` would mean the parameter or the local here: the import is not reused
export function shadowed(impl: string) {
	return probeSwap(3, impl);
}
export function withLocal() {
	const impl = "x";
	return probeSwap(4, impl);
}
// the new import's temporary must not be captured by a local named like it
export function withTemp(impl: number) {
	const _impl = 3;
	return probeSwap(impl, "s") + _impl;
}
