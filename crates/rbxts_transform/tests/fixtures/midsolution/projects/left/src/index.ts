import { make, Shape } from "../../common/src";
import { twice } from "../../common/src/util/math";

export function left(): Shape {
	return make(twice(1));
}
