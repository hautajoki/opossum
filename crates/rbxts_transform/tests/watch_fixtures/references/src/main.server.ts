import { add } from "../projects/common/src/native";
import config from "../projects/external/config.json";
import { add as addExternal } from "../projects/external/helper";
import { add as addFromReference } from "../projects/common/src/external";
import { accumulate, common, Counter, NamedCounter } from "../projects/shared/src";
import type { Named } from "../projects/shared/src";
import { greet } from "./greet";

const counter = new NamedCounter();
const named: Named = counter;
print(greet(named.name), counter instanceof Counter, counter instanceof common.Counter, counter.increment());
print(add(4, 5), addExternal(config.value), addFromReference(config.value));
accumulate([2, 3], value => print(value, common.total));
print(common.label + common.label);
