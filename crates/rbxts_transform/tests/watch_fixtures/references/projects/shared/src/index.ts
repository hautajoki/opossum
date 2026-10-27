import { add, Counter } from "../../common/src";
import type { Named } from "../../common/src";

export { Counter, total } from "../../common/src";
export * as common from "../../common/src";
export type { Named } from "../../common/src";

export function accumulate(values: Array<number>, callback: (value: number) => void) {
	for (const value of values) {
		callback(add(value));
	}
}

export class NamedCounter extends Counter implements Named {
	public name = "shared";
}
