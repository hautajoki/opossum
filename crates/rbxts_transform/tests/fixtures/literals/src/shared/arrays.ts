// Hand-written literals fixture: array literals and array spread.

declare const arr: number[];
declare const readonlyArr: ReadonlyArray<number>;
declare const str: string;
declare const set: Set<number>;
declare const map: Map<string, number>;
declare const gen: Generator<number>;
declare const iter: IterableFunction<number>;
declare const tupleIter: IterableFunction<LuaTuple<[number, string]>>;
declare const shared: SharedTable;
declare const tuple: [number, string];
declare function getArr(): number[];
declare function sideEffect(): number;
declare let n: number | undefined;

export const simple = [1, 2, 3];
export const empty: number[] = [];
export const holes = [1, , 3];
export const nested = [[1], [2, [3]], []];
export const calls = [sideEffect(), sideEffect()];
export const withPrereq = [1, n ?? 2, 3];

export const spreadOnly = [...arr];
export const spreadEnd = [1, 2, ...arr];
export const spreadMiddle = [1, ...arr, 2, 3];
export const spreadTwo = [...arr, ...getArr()];
export const spreadReadonly = [...readonlyArr, 0];
export const spreadString = [...str];
export const spreadStringMiddle = ["a", ...str, "b"];
export const spreadSet = [0, ...set, 1];
export const spreadMap = [...map];
export const spreadGenerator = [...gen, 9];
export const spreadIterable = [...iter];
export const spreadTupleIterable = [1, ...tupleIter, 2];
export const spreadShared = [...shared];
export const spreadTuple = [...tuple, 1];
export const spreadLiteral = [...[1, 2], 3];
export const prereqAfterSpread = [...arr, n ?? 1, 2];
export const prereqBeforeSpread = [n ?? 1, ...arr];
export const spreadStringLiteral = [..."abc"];
export const objectsInArray = [{ a: 1 }, { ...{ b: 2 } }, [...arr]];
