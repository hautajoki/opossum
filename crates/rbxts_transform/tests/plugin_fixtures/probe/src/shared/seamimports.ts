// plugin imports, as Flamework's TS transformer adds them: one declaration per path, the paths in the order their
// first import was asked for, each listing its imports in that order; TypeScript names a second `t` `t_1` in that
// print order, so `./impa`'s `t` is `t` though `./impb`'s was asked for first
export const i1 = probeImportFrom("./impa", "u");
export const i2 = probeImportFrom("./impb", "t");
export const i3 = probeImportFrom("./impa", "t");
export function later() {
	return [probeImportFrom("./impb", "t"), probeImportFrom("./impa", "u")];
}
