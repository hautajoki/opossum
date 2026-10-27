// Hand-written literals fixture: methods in object literals.

declare const record: { x: number };

const key = "k";

export const methods = {
	value: 1,
	method() {
		return this.value;
	},
	arrow: () => 1,
	fn: function () {
		return 2;
	},
	"quoted-name"() {
		return 3;
	},
	[key]() {
		return 4;
	},
	async asyncMethod() {
		return 5;
	},
	*gen() {},
	noSelf(this: void) {
		return 6;
	},
	withArgs(a: number, b = 2, ...rest: number[]) {
		return a + b + rest[0];
	},
};

// once the object is a temporary, methods become declarations
export const afterSpread = {
	...record,
	method() {
		return 1;
	},
	noSelf(this: void) {
		return 2;
	},
	"bad name"() {
		return 3;
	},
	async later() {
		return 4;
	},
	end: true,
};
