// Hand-written T2 exprs fixture: yield and yield*.

export function* counter(n: number) {
	yield;
	yield n;
	const received: unknown = yield n + 1;
	print(received);
	return 5;
}

export function* delegating(n: number) {
	yield* counter(n);
	const result = yield* counter(n + 1);
	print(result);
}

export const arrowLike = function* () {
	const x: number = yield 1;
	yield x * 2;
};
