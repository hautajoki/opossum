/** @jsx createElement */
// Dotted JSX tags are accesses roblox-ts lowers itself: offered to the expression hooks and to `property_key`.
export function dotted() {
	return <tags.probeAccTag />;
}
export function keyedTag() {
	return <keyedTags.Comp />;
}
export function plainTag() {
	return <tags.plainTag />;
}
