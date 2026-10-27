// Hand-written literals fixture: spread in call arguments.

declare const arr: number[];
declare const set: Set<number>;
declare const str: string;
declare const map: Map<string, number>;
declare function take(...args: number[]): void;
declare function takeStrings(...args: string[]): void;
declare function takeAny(...args: unknown[]): void;

take(...arr);
take(1, 2, ...arr);
take(...set);
takeStrings(...str);
takeAny(...map);
take(...[1, 2]);
print(...arr);

export function forward(...args: number[]) {
	take(...args);
	return [...args];
}
