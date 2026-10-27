// An if-expression on the left of `or`: `$range`'s fallback for a nil step. roblox-ts rendered
// `if flag then steps[6] else 2 or 1`, where the `or 1` joins the `else` branch, so a nil step reached the loop.
const steps: number[] = [];

export function rangeStep(flag: boolean) {
	const log: string[] = [];
	for (const i of $range(1, 3, flag ? steps[5] : 2)) log.push(`${i}`);
	return log;
}
