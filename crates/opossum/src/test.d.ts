// opossum/test: types for suites. `import type { Suite } from "opossum/test"`
export interface TestOptions { readonly only?: boolean; readonly skip?: boolean | string }
export interface BenchOptions extends TestOptions {
	/** Total timed calls spread over the samples; omit to calibrate (~5 ms per sample). */
	readonly iterations?: number
	/** Timed samples, default 12. */
	readonly samples?: number
	readonly beforeSample?: () => void
	/** Runs after every attempted sample, even a failed one. */
	readonly afterSample?: () => void
}
export interface Expectation {
	readonly never: Expectation
	readonly toBe: (expected: unknown) => Expectation
	readonly toEqual: (expected: unknown) => Expectation
	readonly toBeNear: (expected: number, tolerance?: number) => Expectation
	readonly toBeTruthy: () => Expectation
	readonly toExist: () => Expectation
	readonly toHaveType: (typeName: string) => Expectation
	readonly toHaveKey: (key: unknown) => Expectation
	readonly toContain: (value: unknown) => Expectation
	/** A Lua pattern. */
	readonly toMatch: (pattern: string) => Expectation
	/** A Lua pattern matched against the whole message, `file:line:` prefix included. */
	readonly toThrow: (pattern?: string) => Expectation
}
export interface SuiteContext {
	readonly describe: (label: string, inner: () => void) => void
	readonly test: (label: string, run: () => void, options?: TestOptions) => void
	/** `index` is 1-based (Luau iteration order). */
	readonly testEach: <T>(values: readonly T[], label: string | ((value: T, index: number) => string),
		run: (value: T, index: number) => void, options?: TestOptions) => void
	readonly bench: (label: string, run: () => void, options?: BenchOptions) => void
	readonly todo: (label: string, reason?: string) => void
	readonly beforeEach: (hook: () => void) => void
	/** Runs LIFO after the case's `cleanup` callbacks, even after a failure. */
	readonly afterEach: (hook: () => void) => void
	/** Registers a callback for the running case; all run once, LIFO, even after a failure. */
	readonly cleanup: (hook: () => void) => void
	readonly expect: (value: unknown) => Expectation
	readonly skip: (reason?: string) => never
	readonly fail: (message?: string) => never
}
export type Suite = (t: SuiteContext) => void
