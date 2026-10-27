// classes without inheritance: fields, parameter properties, methods, statics

export function useClasses() {
	print(NoConstructor, Empty, Shape, WithCallbacks);
}

export class Point {
	public x = 0;
	public y: number;
	public label = "point";
	private unset?: string;

	constructor(y: number, public readonly z: number, protected w = 3) {
		// a comment on the first statement of a constructor without super()
		this.y = y;
		print("made", this.x, this.y);
	}

	public length() {
		return math.sqrt(this.x * this.x + this.y * this.y);
	}

	public scaled(by: number) {
		this.x *= by;
		return this;
	}

	public static origin = 0;
	public static readonly dimensions = 2;

	public static describe(this: void) {
		return "Point";
	}

	public static helper() {
		return this.dimensions + Point.origin;
	}

	static {
		print("static block", this.dimensions);
		Point.origin = 1;
	}

	public ["computed key"]() {
		return 1;
	}

	public "quoted"() {
		return 2;
	}

	public 10() {
		return 10;
	}

	public async later() {
		print("later");
	}

	public overloaded(a: string): string;
	public overloaded(a: number): number;
	public overloaded(a: string | number) {
		return a;
	}

	public toString() {
		return `Point(${this.x}, ${this.y})`;
	}
}

class NoConstructor {
	public value = 5;
	public other = this.value + 1;
	public method() {
		return this.value;
	}
}

class Empty {}

abstract class Shape {
	public abstract area(): number;
	public describe() {
		return `area ${this.area()}`;
	}
}

class WithCallbacks {
	public callback = () => this.value;
	public value = 1;
	public static staticCallback = () => WithCallbacks.counter;
	public static counter = 0;
	public static readonly self = this;
}
