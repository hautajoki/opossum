// A computed key's prerequisites in an optional chain. roblox-ts ran them before the receiver's nil check, so
// `x?.[(kv = 1, "m")]?.()` ran `kv = 1` even when `x` was nil.
let kv = 0;

export function keyPrerequisites(x?: { m?: () => number }) {
	kv = 0;
	const r = x?.[(kv = 1, "m")]?.();
	return [r ?? -1, kv];
}

// a receiver that is not optional: the key comes after it
export function keyAfterReceiver() {
	let holder: { m?: () => string } = { m: () => "first" };
	const second = { m: () => "second" };
	const r = holder[((holder = second), "m")]?.();
	return r;
}
