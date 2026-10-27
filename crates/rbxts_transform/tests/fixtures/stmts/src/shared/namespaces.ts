// Namespaces: roblox-ts tests' namespace.spec.ts (without its object literals), plus hand-written cases.

function readNamespace() {
	return n1.a;
}

interface n1 {
	value: number;
}

declare namespace n1 {
	interface DeclaredShape {
		value: number;
	}
}

namespace TypesOnly {
	export interface Shape {
		value: number;
	}
}

namespace n1 {
	export declare const absent: number;
	export const a = "a";
	export function read(value: string): string;
	export function read(value: string) {
		return value;
	}
	export interface Shape {
		value: number;
	}
	export const Shape = 42;
	export type Label = string;
	export const Label = "label";
}

namespace dotted.inner {
	export const value = 42;
}

namespace n2 {
	export namespace n3 {
		export const a = "a";
	}
}

namespace foo {
	export namespace foo {
		export namespace foo {
			export const bar = "bar";
		}
	}
}

let namespaceEffects = 0;
namespace Empty {
	namespaceEffects++;
}
namespace WithFunction {
	export function value() {
		return 42;
	}
}
namespace Nested.Self {
	export function value() {
		return Self;
	}
}

enum EnumBeforeNamespace {
	Value = 3,
}
namespace EnumBeforeNamespace {
	export interface Shape {
		value: number;
	}
}

namespace NamespaceBeforeEnum {
	export type Label = string;
}
enum NamespaceBeforeEnum {
	Value = 4,
}

// hand-written: mutable exports, enums and private members inside, exported and local namespaces
namespace Counter {
	export let count = 0;
	const step = 1;
	export function increment() {
		count += step;
		return count;
	}
	export enum Kind {
		Up,
		Down,
	}
	export const enum Hidden {
		X = 1,
	}
	export const hiddenValue = Hidden.X;
}

namespace ConstEnumOnly {
	export const enum Values {
		A = 1,
	}
}

declare namespace Ambient {
	export const value: number;
}

export namespace Public {
	export const name = "public";
	export function greet() {
		return name;
	}
	export namespace Inner {
		export const depth = 2;
	}
}

namespace Outer {
	namespace Local {
		export const value = ConstEnumOnly.Values.A + Ambient.value;
	}
	export function makeLocal() {
		return Local.value;
	}
}

export function makeLocal() {
	return Outer.makeLocal();
}

export function run() {
	it("should preserve enums merged with namespaces containing only types", () => {
		const label: NamespaceBeforeEnum.Label = NamespaceBeforeEnum[NamespaceBeforeEnum.Value];

		expect(EnumBeforeNamespace.Value).to.equal(3);
		expect(EnumBeforeNamespace[3]).to.equal("Value");
		expect(label).to.equal("Value");
	});

	it("should execute namespaces without exports", () => {
		expect(namespaceEffects).to.equal(1);
		expect(WithFunction.value()).to.equal(42);
		expect(Nested.Self.value()).to.equal(Nested.Self);
	});

	it("should support namespaces", () => {
		expect(n1.a).to.equal("a");
		expect(readNamespace()).to.equal("a");
		expect(n1.read("value")).to.equal("value");
		expect(n1.Shape).to.equal(42);
		expect(n1.Label).to.equal("label");
		expect(dotted.inner.value).to.equal(42);
	});

	it("should support nested namespaces", () => {
		expect(n2.n3.a).to.equal("a");
	});

	it("should support shadowed namespaces", () => {
		expect(foo.foo.foo.bar).to.equal("bar");
	});

	it("should support mutable namespace exports", () => {
		expect(Counter.increment()).to.equal(1);
		expect(Counter.count).to.equal(1);
		Counter.count = 5;
		expect(Counter.Kind.Down).to.equal(1);
		expect(Counter.hiddenValue).to.equal(Counter.Hidden.X);
		expect(Public.greet()).to.equal(Public.name);
		expect(Public.Inner.depth).to.equal(2);
	});
}
