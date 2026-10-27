// call macros, string and math macros, Promise

export function callMacros(value: unknown, inst: Instance, n: number, s: string, maybe?: Instance) {
	assert(n);
	assert(s, "message");
	assert(inst);
	assert(maybe);
	print(typeOf(value), typeIs(value, "number"), typeIs(value, "Vector3"), classIs(inst, "Part"));
	print(identity<number>(n), identity(s));
	if (typeIs(value, "string")) {
		print(value.upper());
	}
}

export function strings(s: string, n: number, getString: () => string) {
	print(s.size(), s.byte(1), s.find("a"), s.format(n), s.gmatch("%d"), s.gsub("a", "b"), s.lower());
	print(s.match("x"), s.rep(2), s.reverse(), s.split(","), s.sub(1, 2), s.upper());
	print(getString().size(), getString().sub(n));
}

export function maths(a: Vector3, b: Vector3, cf: CFrame, u: UDim2, v2: Vector2, n: number) {
	print(a.add(b), a.sub(b), a.mul(2), a.div(b), a.idiv(2));
	print(cf.mul(a), cf.add(a), cf.sub(a), u.add(u), u.sub(u), v2.mul(v2), v2.div(2), n.idiv(3));
	print(a.add(b).mul(a.sub(b)));
}

export function promises(p: Promise<number>) {
	const q = Promise.resolve(1);
	p.then((v) => print(v));
	return q.then((v) => v + 1);
}

export function trees() {
	return $getModuleTree("./arrays");
}
