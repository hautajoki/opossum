// unusual members: reserved and non-identifier names, generators, declare fields, index signatures, rest parameters

const dynamicKey = "dynamic";

class Members {
	[key: string]: unknown;
	declare declared: number;
	public [dynamicKey] = 1;
	public "with space" = 2;
	public 3 = "three";
	public static [dynamicKey] = 4;

	constructor(first = 1, ...rest: Array<number>) {
		print(first, rest);
	}

	public end() {
		return "end";
	}

	public "not an identifier"() {
		return 1;
	}

	public *generator() {
		return 1;
	}

	public static async staticAsync() {
		return 1;
	}

	public static *staticGenerator() {
		return 2;
	}

	public withThisVoid(this: void) {
		return 3;
	}

	public static staticArrow = () => this;
	public static staticFunction = function () {
		return 1;
	};

	public async ["async computed"]() {
		return 1;
	}
}

class OverloadedConstructor {
	constructor(a: number);
	constructor(a: string);
	constructor(a: number | string) {
		print(a);
	}
}

export function makeLocal() {
	class InFunction {
		public static x = 1;
	}
	{
		class InBlock extends InFunction {
			public y = InFunction.x;
		}
		return InBlock;
	}
}

export function useMembers() {
	print(Members, OverloadedConstructor);
}
