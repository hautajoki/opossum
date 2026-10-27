// A package imported by relative path while it is also a dependency linked into node_modules (tests/fixtures.rs,
// `junction_relative_dependency`): no resolution goes through the link, so TypeScript 5 knows of none and the import
// keeps the package's own path.
import { greet } from "../../packages/greeter";

export const x = greet("r");
