// names the plugin declared after is_usable_local_name approved them are not taken by seam temporaries or imports
export function f() {
	const a = probeApprove(1);
	return a + probeHoist(2);
}
export function g() {
	const b = probeApproveImpl(1);
	return b + probeSwap(3, "s");
}
