// `join` on an optional chain. roblox-ts read the receiver's type with `undefined` in it, found no element type, and
// skipped the `tostring` loop, so `table.concat` failed on non-string values.
export function optionalBooleans(flags?: Array<boolean>) {
	return flags?.join(",");
}

// strings need no loop, with or without the optional chain
export function optionalStrings(names?: Array<string>) {
	return names?.join("+");
}
