import { probeElide, helper } from "./macros";
import { probeKeep } from "./macros";

export const a = probeElide();
export const b = helper;
export const c = probeKeep();
export const late = probeLate();
