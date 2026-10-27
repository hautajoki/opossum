// Function values in slots whose signature has a `this` parameter. TypeScript 5 gives a function expression or
// object literal method without an explicit `this` the contextual signature's `this`; TypeScript 7 only when the
// function uses `this` or has an unannotated parameter.

export function accept(callback?: MethodSlot) {
	return callback;
}

type MethodSlot = (this: defined, arg?: string) => void;
type CallbackSlot = (this: void, arg?: string) => void;

// a parameterless function expression in a method slot (roblox-ts's array.spec)
export const parameterless: Array<MethodSlot> = [
	function () {
		print("parameterless");
	},
	function (arg) {
		print(arg);
	},
];

// only annotated parameters: TypeScript 7 alone reported "expected method, got function" here
export const annotated: MethodSlot = function (arg?: string) {
	print(arg);
};

// through an optional parameter, whose contextual type is a union with undefined
accept(function (arg?: string) {
	print(arg);
});
accept(function () {});

// object literal members against their contextual property types
export const holder: { method: MethodSlot; callback: CallbackSlot; short: CallbackSlot } = {
	method: function (arg?: string) {
		print(arg);
	},
	callback: function (arg?: string) {
		print(arg);
	},
	short(arg?: string) {
		print(arg);
	},
};
holder.method("x");
holder.callback("y");
holder.short("z");

// neighbours that must not change
export const arrow: MethodSlot = () => print("arrow");
export const usesThis: MethodSlot = function (arg?: string) {
	print(this, arg);
};
export const explicitThis: MethodSlot = function (this: defined, arg?: string) {
	print(arg);
};
export const voidSlot: CallbackSlot = function (arg?: string) {
	print(arg);
};
export const plain = function (arg?: string) {
	print(arg);
};
