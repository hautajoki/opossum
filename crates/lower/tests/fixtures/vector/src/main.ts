declare const v: Vector3;
declare const w: Vector3;
declare const maybe: number | undefined;
declare const either: Vector3 | Vector2;
declare function receiver(): Vector3;
declare function argument(): Vector3;
declare namespace Userland {
	interface Vector3 {
		readonly Magnitude: number;
	}
}
declare const lookalike: Userland.Vector3;
declare const a: vector;

function shadow() {
	const vector = (x: number) => new Vector3(x, 2);
	return vector(1);
}

print(shadow(), new Vector3(maybe), Vector3.zero, Vector3.xAxis);
print(v.Unit.Dot(w), v.Max(w, v), v.Lerp(w, 0.5), either.Magnitude, lookalike.Magnitude);
let current = Vector3.zero;
print(receiver().Dot((current = argument())), current);
print(a.add(a).mul(2), vector.lerp(a, a, 0.5));
