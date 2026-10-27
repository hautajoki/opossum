import type { Shape } from "shapes";

export function describe(shape: Shape): string {
	return `${shape.kind}:${shape.size}`;
}
