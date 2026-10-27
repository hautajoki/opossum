// Computed keys a plugin rewrote (probe's `property_key`): an argument that only reads is dropped with the key, one
// with effects still runs once, after the receiver.
enum DK {
	A = "a",
}
export function enumKey() {
	return dropKeyed[DK.A];
}
export function callKey() {
	return getDropKeyed()[(keyName() as "a")];
}
export function readKey() {
	return dropKeyed[dropKeyed.s as "a"];
}
export function writeKey() {
	getDropKeyed()[DK.A] = 2;
	return dropKeyed.a;
}
