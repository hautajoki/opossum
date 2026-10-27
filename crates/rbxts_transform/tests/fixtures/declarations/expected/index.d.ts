/// <reference types="@rbxts/types" preserve="true" />
import { Vector } from "./util/math";
import type { Options } from "./shapes";
export * from "./util/math";
export { describe } from "./util/describe";
export type { Shape } from "./shapes";
export declare function configure(options: Partial<Options>): Options;
export declare function origin(): Vector;
export declare function playerCount(): number;
/** A counter with a limit. */
export declare class Counter {
    private readonly limit;
    private value;
    constructor(limit: number);
    increment(by?: number): number;
    current(): number;
}
export declare const enum Mode {
    Fast = 0,
    Slow = "slow"
}
export type Loader = typeof import("./util/math");
export default Counter;
//# sourceMappingURL=index.d.ts.map