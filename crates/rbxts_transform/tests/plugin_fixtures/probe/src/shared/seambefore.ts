// a root statement's leading comment: insert_before_statement goes above it
export const b1 = probeBefore(1);

export function f(x: number) {
	// inside a function body
	const y = probeBefore(x + 1);
	if (x > 0) return probeBefore(y);
	else if (probeBefore(x) < -1) {
		print("negative");
	}
	while (probeBefore(x) > 10) {
		x--;
	}
	switch (x) {
		case 1:
			print(probeBefore("case"));
			break;
	}
	const arrow = () => probeBefore(3);
	return arrow;
}

export class Fields {
	field = probeBefore(4);
}

// a plugin comment leading its statement's Luau goes above the statement's leading comment
export const p1 = probePrereq(5);

// several insertions before one statement go in TypeScript's visit order (source order), not roblox-ts's lowering
// order: a `for` head's condition before its incrementor, a class's members as written (a static field before a
// method's parameter initializer and a later instance field)
export function loop() {
	for (let i = probeBefore(0); i < probeBefore(3); i += probeBefore(1)) {
		print(i);
	}
}
export class Ordered {
	static first = probeBefore("static first");
	method(a = probeBefore("parameter")) {
		return a;
	}
	second = probeBefore("instance second");
	static third = probeBefore("static third");
}
