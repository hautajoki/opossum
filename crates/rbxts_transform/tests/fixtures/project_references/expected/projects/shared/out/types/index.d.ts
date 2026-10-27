import { Counter } from "../../common/src/index";
import type { Named } from "../../common/src/index";
export { Counter, total } from "../../common/src/index";
export * as common from "../../common/src/index";
export type { Named } from "../../common/src/index";
export declare function accumulate(values: Array<number>, callback: (value: number) => void): void;
export declare class NamedCounter extends Counter implements Named {
    name: string;
}
