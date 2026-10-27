// The same order for type reference directives (tests/fixtures.rs, `junction_interleaved_type_refs`): this file's
// references to `@rbxts/hub` and `@rbxts/zeta` come before the hub's reference to `@rbxts/alpha`, so the import of
// the greeter by relative path goes through `zeta`.
/// <reference types="@rbxts/hub" />
/// <reference types="@rbxts/zeta" />
import { greet } from "../../vendor/greeter";

export const r = greet("r");
