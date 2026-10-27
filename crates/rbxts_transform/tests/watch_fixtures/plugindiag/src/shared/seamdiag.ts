// plugin diagnostics are kept once in the build: one identical to roblox-ts's own warning at the same node, and
// one at a node in another file, which two files report
declare const count: number;
export function check() {
	if (probeDiagSame(count, "Value will be checked against 0, NaN")) {
		return 1;
	}
	return probeDiagDecl();
}
