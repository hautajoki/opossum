import { impl, other } from "./runtime";

type Impl = typeof impl;
export const value: Impl | number = other;
export const swapped = probeSwap(2, "two");
