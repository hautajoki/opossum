import type { DI } from "./d";
declare module "./d" {
	interface DI {
		x: number;
	}
}
export const aug = (v: DI) => v.x + v.k;
