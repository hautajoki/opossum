let counter = 0;
function bump() {
	counter += 1;
	return counter;
}
declare const maybe: number | undefined;

export const hoisted = probeHoist([1, 2]);
export function useHoist() {
	return probeHoist({ size: counter });
}
export const prereq = probePrereq(bump());
export const ordered = probeOrder(counter, maybe ?? bump());
probeInject(bump());
export const swapped = probeSwap(1, `n${counter}`);
export const captured = probeCapture(maybe ?? bump());
probeRoot();
export const late = [probeLate(), probeLate()];
export const stamped = here();
export const built = buildId();
export const unknown = mystery();
export const plain = bump();
