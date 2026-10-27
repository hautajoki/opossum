// A server script using the fixture modules.
import { plain } from "shared/conditional";
import { counter } from "shared/generators";

print(plain(true, 1, 2), counter(1));
