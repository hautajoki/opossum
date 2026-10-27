import { alpha } from "shared/variables";
import { objectParameter } from "shared/parameters";
import { swap } from "shared/assignments";
import { strings } from "shared/iterables";

try {
	print(alpha, objectParameter({ a: 1 }), swap(), strings());
} catch (e) {
	print(e);
}
