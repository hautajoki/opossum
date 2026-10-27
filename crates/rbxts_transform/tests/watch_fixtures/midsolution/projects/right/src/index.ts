import { bump, count } from "../../common/src";

export function right() {
	bump();
	return count;
}

export function check() {
	if (count) {
		print(count);
	}
}
