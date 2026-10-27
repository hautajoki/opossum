// A hand-written declaration file: an asset, printed again by TypeScript's printer.
import type { Vector } from "./util/math";
export interface Options {
    name: string;
    size: number;
}
export interface Shape {
    kind: "circle" | 'square';
    size: number;
    center?: Vector; // trailing comment
}
/** Documented. */
export declare function makeShape(kind: Shape["kind"]): Shape;
