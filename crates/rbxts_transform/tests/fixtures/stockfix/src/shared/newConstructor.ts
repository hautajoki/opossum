// `new` evaluates its constructor before the arguments. roblox-ts read `C` after the arguments' prerequisites ran,
// so `new C((C = B, "arg"))` constructed a `B`.
class A {
	constructor(public tag: string) {}
	name = "A";
}
class B {
	constructor(public tag: string) {}
	name = "B";
}

export function reassigned() {
	let C: typeof A | typeof B = A;
	const made = new C((C = B, "arg"));
	return [made.name, made.tag, new C("again").name];
}

// nothing in the arguments changes the constructor: no capture
export function plain(tag: string) {
	return new A(tag).tag;
}
