// for-of evaluation order. roblox-ts evaluated a target's prerequisites once, before the loop and before the
// iterable; applied a nested pattern's default after destructuring the value; and read the iterable before its own
// prerequisites when the loop builder captured it.
let n = 0;
let log: string[] = [];
function key() {
	log.push(`key${n}`);
	return n++;
}
function list() {
	log.push("list");
	return [10, 20];
}

// the target is evaluated on every iteration, after the iterable
export function writableTarget() {
	n = 0;
	log = [];
	const obj: Record<number, number> = {};
	for (obj[key() + n++] of list()) log.push("body");
	return [log.join(" "), obj[1], obj[5]];
}

// the same through a map loop's pair
function map() {
	log.push("map");
	return new Map([["a", 1]]);
}
export function writableMapTarget() {
	n = 0;
	log = [];
	const obj: Record<number, [string, number]> = {};
	for (obj[key() * 10 + n++] of map()) log.push("body");
	return [log.join(" "), obj[1][0], obj[1][1]];
}

declare function entries(): IterableFunction<LuaTuple<[number, [number] | undefined]>>;

// the default applies before the nested pattern reads the value
export function nestedDefault() {
	const out: number[] = [];
	for (const [k, [v] = [7]] of entries()) out.push(k, v);
	return out;
}

declare function makeIter(count: number | undefined): IterableFunction<LuaTuple<number[]>>;
const box: { count?: number } | undefined = { count: 2 };

// the iterable's own prerequisites come before the builder captures it
export function iterablePrerequisites() {
	const out: number[] = [];
	for (const t of makeIter(box?.count)) out.push(t[0]);
	return out;
}
