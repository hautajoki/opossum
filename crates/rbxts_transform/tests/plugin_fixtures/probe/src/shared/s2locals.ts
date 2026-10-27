const top1 = 1;
const top2 = 2;
export const atTop = probeLocals();
export function inFunction() {
	const a = 1;
	const [b, c] = [2, 3];
	if (a > 0) {
		const nested = 4;
		print(nested);
	}
	return probeLocals() + a + b + c + top1 + top2;
}
// a call claimed inside parentheses of a `for` head
for ((probeLocals()); ; ) {
	break;
}
// a claimed literal indexed or called: parenthesized
export const lit = (probeLocals() as unknown as { p: number }).p;
