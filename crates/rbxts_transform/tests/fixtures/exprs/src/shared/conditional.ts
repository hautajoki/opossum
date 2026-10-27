// Hand-written T2 exprs fixture: conditional expressions.

declare function sideEffect(): number;
declare function check(): boolean;

export function plain(flag: boolean, a: number, b: number) {
	return flag ? a : b;
}

// truthiness checks on a number or string condition
export function truthy(n: number, s: string) {
	const x = n ? "yes" : "no";
	const y = s ? 1 : 2;
	return x + y;
}

// a branch with prerequisites becomes an if statement and a temporary
export function withPrereqs(flag: boolean, n: number) {
	let i = n;
	const v = flag ? i++ : i--;
	return v + i;
}

// used as a statement
export function asStatement(flag: boolean) {
	flag ? sideEffect() : check();
	let count = 0;
	flag ? count++ : count--;
	return count;
}

// nested conditionals
export function nested(a: boolean, b: boolean) {
	return a ? (b ? 1 : 2) : b ? 3 : 4;
}

// condition with prerequisites
export function conditionPrereqs(n: number) {
	let i = n;
	return i++ > 2 ? i : -i;
}

export function inCall(flag: boolean) {
	print(flag ? "a" : "b", sideEffect());
}
