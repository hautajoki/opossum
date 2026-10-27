// extends, super calls, super methods, implicit constructors

export function useInheritance() {
	print(Derived, ImplicitDerived, EmptyDerived, AbstractDerived, FromCall, Implementer, SuperIndexed);
}

export function useMore() {
	print(NestedSuper, SuperNotFirst, FromExpression, SuperMiddle);
}

class Base {
	public name = "base";
	constructor(public readonly id: number) {}

	public greet(greeting: string) {
		return `${greeting} ${this.name}`;
	}

	public static create(this: void, id: number) {
		return id;
	}

	public static count() {
		return 1;
	}
}

class Derived extends Base {
	public extra = 1;

	constructor(id: number, public readonly tag: string) {
		print("before super");
		super(id);
		print("after super", this.extra);
	}

	public greet(greeting: string) {
		return super.greet(greeting) + "!";
	}

	public static count() {
		return super.count() + 1;
	}
}

class ImplicitDerived extends Derived {
	public more = "x";
}

class EmptyDerived extends Base {}

abstract class AbstractDerived extends Base {
	public abstract run(): void;
}

function getBase() {
	return Base;
}

class FromCall extends getBase() {
	constructor() {
		super(2);
	}
}

interface Greeter {
	greet(greeting: string): string;
}

class Implementer implements Greeter {
	public greet(greeting: string) {
		return greeting;
	}
}

class SuperIndexed extends Base {
	public greet(greeting: string) {
		// comment before the super call
		return super["greet"](greeting);
	}
}

class NestedSuper extends Base {
	public field = "field";
	constructor(flag: boolean) {
		if (flag) {
			super(1);
		} else {
			super(2);
		}
		print(this.field);
	}

	public static make() {
		return super.create(5);
	}
}

class SuperNotFirst extends Base {
	constructor(public readonly label: string) {
		const id = 3;
		super(id);
	}
}

const BaseExpression = class extends Base {};

class FromExpression extends BaseExpression {}

export default class NamedDefaultClass extends Base {
	constructor() {
		super(4);
	}
}

class SuperMiddle extends Base {
	public q = 1;
	constructor() {
		// before super
		super(1);
		// set up
		print("x");
		// trailing
	}
}
