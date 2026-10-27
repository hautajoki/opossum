// Enums: roblox-ts tests' enum.spec.ts (without its object literal case), plus hand-written cases.

// Basic enum test
enum Fruits {
	Apple,
	Orange,
	Pear,
	$,
}

// Numeric enum test
enum Breads {
	White = 5,
	Wheat,
	Pumpernickel,
	$,
}

// String enums
enum Soups {
	Tomato = "TOMATO",
	ChickenNoodle = "CHICKENNOODLE",
	Dumpling = "DUMPLING",
	$ = "DOLLARS",
}

enum EnumWithEscapedQuote {
	// prettier-ignore
	Quote = "\"",
}

const enum EnumWithEscapedQuoteConst {
	// prettier-ignore
	Quote = "\"",
}

const enum Person {
	Validark,
	Osyris,
	Evaera,
	Vorlias,
	DataBrain,
	$,
}

const enum Animal {
	Bear = "BEAR",
	Dog = "DOG",
	Snake = "SNAKE",
	$ = "SCARAB",
}

function getValue() {
	return 123;
}

enum WithInitializer {
	value = getValue(),
}

// hand-written: mixed values, references between members, negative and fractional values, quoted names
enum Mixed {
	A = 1,
	B = A * 2,
	C = "c",
	D = -3,
	E = 0.5,
	"quoted name" = 7,
	F = Fruits.Pear,
	G = Breads.Wheat | 8,
	H = ~1,
}

export enum Exported {
	One = 1,
	Two,
}

export const enum ExportedConst {
	Hidden = "hidden",
}

function readLater() {
	return Later.First + Later.Second;
}

enum Later {
	First = 10,
	Second,
}

export function useConstNumbers() {
	return Person.Osyris + Person["Evaera"];
}

export function useConstStrings() {
	return Animal.Dog + ExportedConst.Hidden + EnumWithEscapedQuoteConst.Quote;
}

export function run() {
	it("should expose enums by number", () => {
		expect(Fruits[0]).to.equal("Apple");
		expect(Fruits.Orange).to.equal(1);
		expect(Fruits[2]).to.equal("Pear");
	});

	it("should support overriding indices", () => {
		expect(Breads[5]).to.equal("White");
		expect(Breads.Wheat).to.equal(6);
		expect(Breads[0]).never.to.be.ok();
	});

	it("should support for string indices", () => {
		expect(Soups.Tomato).to.equal("TOMATO");
		expect(Soups.ChickenNoodle).to.equal("CHICKENNOODLE");
		expect(Soups.Dumpling).to.equal("DUMPLING");
		expect(Soups.$).to.equal("DOLLARS");
	});

	it("should support numeric const enums", () => {
		expect(Person.Validark).to.equal(0);
		expect(Person.Osyris).to.equal(1);
		expect(Person["Osyris"]).to.equal(1);
		expect(Person.$).to.equal(5);
	});

	it("should support string const enums", () => {
		expect(Animal.Bear).to.equal("BEAR");
		expect(Animal["Dog"]).to.equal("DOG");
		expect(Animal.$).to.equal("SCARAB");
	});

	it("should support members with initializers", () => {
		expect(WithInitializer.value).to.equal(123);
	});

	it("should support hoisted enums", () => {
		expect(Animal2.Bear).to.equal("BEAR");
		expect(Animal2.Dog).to.equal("DOG");
		expect(Animal2.Snake).to.equal("SNAKE");
		expect(readLater()).to.equal(21);
	});

	enum Animal2 {
		Bear = "BEAR",
		Dog = "DOG",
		Snake = "SNAKE",
	}

	it("should support enums with escaped quotes", () => {
		expect(EnumWithEscapedQuote.Quote).to.equal('"');
	});

	it("should support const enums with escaped quotes", () => {
		expect(EnumWithEscapedQuoteConst.Quote).to.equal('"');
	});

	it("should support mixed enums", () => {
		expect(Mixed.B).to.equal(2);
		expect(Mixed[Mixed.D]).to.equal("D");
		expect(Mixed["quoted name"]).to.equal(7);
		expect(Exported[Exported.Two]).to.equal("Two");
	});

	it("should support computed keys", () => {
		const constKeyIndex = 4;
		let constKeyValue = 7;
		enum ConstComputedKey {
			// @ts-expect-error computed enum property key
			[constKeyIndex] = constKeyValue,
		}
		expect(ConstComputedKey[4]).to.equal(7);

		let computedKeyIndex = 1;
		enum ComputedKey {
			// @ts-expect-error computed enum property key
			[computedKeyIndex++] = computedKeyIndex,
		}
		expect(ComputedKey[1]).to.equal(2);
		expect(computedKeyIndex).to.equal(2);

		let computedValueIndex = 3;
		enum PrereqInValue {
			// @ts-expect-error computed enum property key
			[computedValueIndex] = (computedValueIndex = 6),
		}
		expect(PrereqInValue[3]).to.equal(6);
		expect(computedValueIndex).to.equal(6);

		enum StringKey {
			["literal key"] = 1,
			["other key"] = "two",
		}
		expect(StringKey["literal key"]).to.equal(1);
	});
}
