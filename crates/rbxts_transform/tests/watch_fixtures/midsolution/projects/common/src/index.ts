export interface Shape {
	kind: "a" | "b";
	size: number;
}

export function make(size: number): Shape {
	return { kind: "a", size };
}

export function part(): Instance {
	return new Instance("Part");
}

export let count = 0;
export const joined = SEP + SEP;

export function nonzero() {
	if (count) {
		return 1;
	}
	return 0;
}

export function bump() {
	return ++count;
}
