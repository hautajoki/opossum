// class expressions, hoisting, self-references

const Anonymous = class {
	public value = 1;
};

const Named = class Inner {
	public static instance = Inner;
	public static other = this;
	public method() {
		return Inner;
	}
};

function takesClass(value: object) {
	return value;
}

takesClass(
	class {
		public static x = 1;
	},
);

takesClass(
	class Local extends Named {
		public static y = Local;
	},
);

function usesLater() {
	return Later;
}

class Later {
	public static value = 1;
}

export function useExpressions() {
	print(Anonymous, Named, usesLater());
}
