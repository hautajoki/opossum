// Hand-written T2 exprs fixture: `||=`, `&&=`, `??=`.

interface Holder {
	n?: number;
	s: string;
	b: boolean;
	child: Holder;
}

declare function getHolder(): Holder;
declare function key(): string;
declare function make(): number;

export function identifiers(x: number | undefined, y: string, z: boolean) {
	x ??= 5;
	y ||= "default";
	z &&= false;
	print(x, y, z);
}

export function properties(h: Holder) {
	h.n ??= 1;
	h.s ||= "s";
	h.b &&= h.s === "x";
	getHolder().child.n ??= make();
}

export function elements(r: Record<string, number | undefined>) {
	r[key()] ??= 2;
	r["a"] ||= 3;
}

// used as values
export function asValues(h: Holder, x: number | undefined) {
	const a = (x ??= 7);
	const b = (h.s ||= "q");
	print(a, b, (h.b &&= true));
	return (h.child.n ??= make());
}

// a right side with prerequisites
export function rightPrereqs(h: Holder) {
	let i = 0;
	h.n ??= i++;
	h.s ||= `${i++}`;
	return i;
}
