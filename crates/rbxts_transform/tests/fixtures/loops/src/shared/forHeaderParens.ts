// Parenthesized expressions in a `for` header's initializer and incrementor (roblox-ts lowers them without the
// expression statement's `skipDownwards`). An unused optional chain or a value-less macro call there is no value:
// stock roblox-ts crashes on it ("Cannot render None"); rbxts-rs drops it like the same statement without parentheses.
declare const maybe: { f(): number } | undefined;
declare function g(): number;

export function optionalChainInitializer() {
	for ((maybe?.f()); ; ) {
		break;
	}
}

export function optionalChainIncrementor() {
	for (; g() < 1; (maybe?.f())) {
		g();
	}
}

export function doubleParentheses() {
	for (((maybe?.f())); g() < 2; ((maybe?.f()))) {
		g();
	}
}

export function macroWithoutValue() {
	const arr = [1];
	for ((arr.push(2)); ; ) {
		break;
	}
}

export function optionalMacroWithoutValue(o: { a?: Array<number> }) {
	for ((o.a?.push(1)); ; ) {
		break;
	}
}

// with a value, stock keeps it, as `local _ = …`
export function valueInParentheses() {
	for ((g()); ; ) {
		break;
	}
}

export function assignmentInParentheses() {
	let x = 0;
	for ((x = 1); ; (x++)) {
		break;
	}
}
