export function pair(): [number, string] {
	return [1, "a"];
}

export function luaTuple(): LuaTuple<[number, string, boolean]> {
	return $tuple(1, "a", true);
}

export function hoisted() {
	function read() {
		return h0 + h1;
	}
	const [h0, h1] = [1, 2];
	return read();
}

export function sideEffect(): "name" {
	print("key");
	return "name";
}

export const { alpha, beta: renamed } = { alpha: 1, beta: "b" };
export let [firstLet, secondLet] = pair();
export let { gamma } = { gamma: true };

interface Config {
	name: string;
	size?: number;
	nested: { inner: number; list: Array<number> };
	["key with space"]: boolean;
	7: string;
}

const config: Config = { name: "n", nested: { inner: 1, list: [1, 2, 3] }, ["key with space"]: true, 7: "seven" };

// one named property reads the receiver once
const { name } = config;
const { name: configName } = config;
const { size = 10 } = config;
const { nested: whole } = { nested: config.nested };

// several properties, nested patterns, defaults, string and numeric keys
const {
	name: n2,
	size: s2 = 5,
	nested: { inner, list: [l1, , l3 = 0] },
	["key with space"]: spaced,
	7: seven,
} = config;

// object rest
const { name: restName, ...others } = config;
const { ...everything } = config;
const { name: oneKey, size: twoKeys, ...remaining } = config;

// computed keys
const key = "name";
const { [key]: viaConst, [sideEffect()]: viaCall, ...afterComputed } = config;

// empty patterns
const {} = config;
const [] = [1, 2];

// array patterns over arrays and tuples
const arr = [1, 2, 3, 4];
const [a0, a1] = arr;
const [, b1, , b3] = arr;
const [c0, ...cRest] = arr;
const [e0] = [...arr];
const [t0, t1] = pair();
const [[p0, p1], [q0]] = [pair(), arr];

// LuaTuple calls and array literals declare all names at once
const [u0, u1, u2] = luaTuple();
const [v0, , v2 = false] = luaTuple();
const [w0, w1] = [1, 2];
const [x0 = 3, [x1] = [4]] = [arr[0], arr];
const [y0, { name: y1 }] = [1, config];
const [z0, ...zRest] = [1, 2, 3];

print(alpha, renamed, firstLet, secondLet, gamma, name, configName, size, whole, n2, s2, inner, l1, l3, spaced, seven);
print(restName, others, everything, oneKey, twoKeys, remaining, viaConst, viaCall, afterComputed);
print(a0, a1, b1, b3, c0, cRest, e0, t0, t1, p0, p1, q0, u0, u1, u2, v0, v2, w0, w1, x0, x1, y0, y1, z0, zRest);
