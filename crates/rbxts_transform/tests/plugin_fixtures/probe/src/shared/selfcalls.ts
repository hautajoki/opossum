// compile_expr of the claimed call itself: lowered as roblox-ts lowers it, on the receiver already compiled
let n = 0;
export const s1 = getChain().s(sideEffect());
export const s2 = maybeChain?.s((n += 1));
export const s3 = getChain().inner.s(2);
export const s4 = probeSelf?.(3);
export const s5 = probeSelf(4);
