// object literal methods in `this: void` slots: TypeScript 5 gives them the slot's `this`, so roblox-ts accepts them
// as callbacks (no "Attempted to assign method where non-method was expected"); TypeScript 7 alone would not
const t: { f: (this: void) => void } = { f() {} };
export const tv = t;
const u: { f: (this: void, n: number) => void } = { f(n: number) {} };
export const uv = u;
