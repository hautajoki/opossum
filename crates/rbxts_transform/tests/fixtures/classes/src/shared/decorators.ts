// experimentalDecorators: class, method, property, parameter and static decorators

export function useDecorators() {
	mutableDecorator = classDecorator;
	print(Stacked, ConstructorParameters, Mutable, ComputedKeys, DecoratedWithConstructor, Expression);
}

function classDecorator(target: object) {
	print("class", target);
}

function classFactory(name: string) {
	print("factory", name);
	return (target: object) => {
		print("class decorator", name, target);
	};
}

function methodDecorator(target: object, key: string, descriptor: TypedPropertyDescriptor<() => void>) {
	print("method", target, key, descriptor);
}

function methodFactory(order: number) {
	return (target: object, key: string, descriptor: TypedPropertyDescriptor<(a: number) => void>) => {
		print("method decorator", order, target, key, descriptor);
	};
}

function propertyDecorator(target: object, key: string) {
	print("property", target, key);
}

function parameterDecorator(target: object, key: string | undefined, index: number) {
	print("parameter", target, key, index);
}

let mutableDecorator = (target: object) => {
	print("mutable", target);
};

@classDecorator
export class Simple {}

@classFactory("first")
@classFactory("second")
@classDecorator
class Stacked {
	@propertyDecorator
	public field = 1;

	@propertyDecorator
	public static staticField = 2;

	@methodDecorator
	public method() {}

	@methodFactory(1)
	@methodFactory(2)
	public withParameters(@parameterDecorator a: number) {}

	@methodDecorator
	public static staticMethod() {}

	public plain() {}
}

class ConstructorParameters {
	constructor(@parameterDecorator first: number, @parameterDecorator second: string) {}

	public method(@parameterDecorator a: number, b: number, @parameterDecorator c: number) {}
}

@mutableDecorator
@mutableDecorator
class Mutable {}

const key = "dynamic";

class ComputedKeys {
	@methodDecorator
	public [key]() {}

	@methodDecorator
	public ["literal key"]() {}
}

@classDecorator
class DecoratedWithConstructor {
	constructor(@parameterDecorator value: number) {}
}

const Expression = class {};
