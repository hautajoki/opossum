// Statements roblox-ts lowers outside its statement dispatch, offered to the statement hooks: a block body (`if`,
// loops, `case`, a static block), an `else if`, and a `declare` statement (which roblox-ts emits nothing for).
declare const probeDeclared: number;
export function blockBody(flag: boolean) {
	if (flag) {
		"probeBlock";
		return 1;
	}
	return 2;
}
export function elseIf(a: boolean) {
	if (a) {
		return "a";
	} else if (probeElseIf) {
		return "b";
	}
	return "c";
}
export function loopBody() {
	let total = 0;
	for (const x of [1, 2]) {
		"probeBlock";
		total += x;
	}
	return total;
}
export function caseBody(n: number) {
	switch (n) {
		case 1: {
			"probeBlock";
			return "one";
		}
		default:
			return "other";
	}
}
export class WithStatic {
	static value = 1;
	static {
		"probeBlock";
		WithStatic.value = 2;
	}
}
