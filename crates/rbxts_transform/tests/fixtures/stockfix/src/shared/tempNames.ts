// Temporary names that would capture or hide the program's own names (roblox-ts named each temporary as the
// program's identifier).
declare function maybe(): { value: number } | undefined;

// a case block's `_fallthrough` hid the switch's flag from `_fallthrough = true`, so case 2 did not run
export function fallthroughCapture(x: number) {
	const log: string[] = [];
	switch (x) {
		case 1: {
			const _fallthrough = "user";
			log.push(_fallthrough);
		}
		case 2:
			log.push("two");
			break;
	}
	return log;
}

// a temporary met only in a `for … in` expression got the fallback name `_0`, which hid the outer `_0`
export function fallbackCapture() {
	const _0 = 10;
	const out: number[] = [];
	for (const v of (() => {
		const r = maybe()?.value;
		return [r ?? _0];
	})())
		out.push(v);
	return out;
}

// a temporary in a loop body named as the loop variable, which the body reads after it
export function loopVariable(list: Array<{ inner?: { value: number }; base: number }>) {
	const out: number[] = [];
	for (const _n of list) {
		const n = _n.inner?.value;
		out.push((n ?? 0) + _n.base);
	}
	return out;
}
