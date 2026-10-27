// Two links to one package (tests/fixtures.rs, `junction_two_links`): imports of either go through the link
// TypeScript 5 met first, `@rbxts/zeta`.
import { greet } from "@rbxts/zeta";
import { greet as g2 } from "@rbxts/alpha";

export const x = greet("z") + g2("a");
