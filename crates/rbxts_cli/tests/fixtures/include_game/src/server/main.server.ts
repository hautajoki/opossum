import { greet, twice } from "shared/greet";

print(greet("server"));
twice(21).then((value) => print(value));
