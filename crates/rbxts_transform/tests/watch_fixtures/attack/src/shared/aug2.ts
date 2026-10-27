import "./b";
import { T } from "./t";

declare module "./b" {
	interface A {
		z: T;
	}
}
