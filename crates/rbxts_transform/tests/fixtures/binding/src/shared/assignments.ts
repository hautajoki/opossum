export function luaTuple(): LuaTuple<[number, number]> {
	return $tuple(1, 2);
}

export function getIndex() {
	print("index");
	return 0;
}

export function swap() {
	let a = 1;
	let b = 2;
	[a, b] = [b, a];
	return a + b;
}

export function assignments() {
	let a = 1;
	let b = 2;
	const obj = { x: 1, y: 2, list: [1, 2, 3] };
	let x = 0;
	let y = 0;

	// one named property and an identifier target
	({ x } = obj);
	({ x: a } = obj);
	({ x = 9 } = obj);
	({ x: b = 8 } = obj);

	// general object patterns
	({ x, y } = obj);
	({ x: a, y: b = 5 } = obj);

	// property and element targets
	const target = { p: 0, q: 0 };
	const list = [0, 0, 0];
	[target.p, target.q] = [x, y];
	({ x: target.p } = obj);
	[list[getIndex()], list[1]] = obj.list;
	({ y: list[getIndex()] } = obj);

	// rest
	let rest: Array<number> = [];
	[a, ...rest] = obj.list;
	let objRest: { y: number; list: Array<number> };
	({ x, ...objRest } = obj);

	// LuaTuple calls
	[a, b] = luaTuple();
	[, b] = luaTuple();
	[target.p, list[getIndex()]] = luaTuple();

	// array literals with calls and effects
	[a, b] = [luaTuple()[0], b];
	[x, y, a] = [1, ...obj.list];

	// used as expressions
	const result = ([a, b] = obj.list);
	const result2 = ({ x, y } = obj);
	const result3 = ({ x } = obj);
	const empty = ([] = obj.list);
	const emptyObject = ({} = obj);
	[] = [];
	({} = {});

	// nested patterns and defaults
	let n0 = 0;
	let n1 = 0;
	[n0, [n1]] = [1, [2]];
	[n0 = 4, n1] = obj.list;
	[n0, { y: n1 }] = [1, obj];
	({ list: [n0, n1 = 7] } = obj);
	({ x: n0, list: [, n1] } = obj);

	// nested patterns below a rest element
	const source: [number, ...Array<Array<number>>] = [1, [42], [99]];
	[n0, ...[[n1]]] = source;
	[a, ...[b, x]] = obj.list;

	return [a, b, x, y, rest, objRest, result, result2, result3, empty, emptyObject, n0, n1];
}
