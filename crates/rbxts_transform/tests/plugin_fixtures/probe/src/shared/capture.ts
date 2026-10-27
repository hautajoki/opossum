// a hoist is declared at the top of the file: a local of the same name in a nested function must not capture it
export function f() {
	const _hoisted = 5;
	return probeHoist(1) + _hoisted;
}
export function g() {
	const _hoisted_1 = 6;
	return probeHoist(2) + _hoisted_1;
}
