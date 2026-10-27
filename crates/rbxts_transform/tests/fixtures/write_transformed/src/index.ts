// Leading comment.
import { helper } from "./helper";
export function f(a: number,b: string): string {
  if (a > 1) { return b }


  return `${a}${b}`;
}
export const value = helper(1)+2; // trailing
