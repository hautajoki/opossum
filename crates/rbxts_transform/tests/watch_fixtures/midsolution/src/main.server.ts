import { left } from "../projects/left/src";
import { right } from "../projects/right/src";
import { make, part } from "../projects/common/src";

const n = right();
if (n) {
	print(n);
}
print(left().size, right(), make(3).kind, part().Name);
