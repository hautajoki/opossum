-- Compiled with roblox-ts v3.0.0
-- Enums: roblox-ts tests' enum.spec.ts (without its object literal case), plus hand-written cases.
-- Basic enum test
local Fruits
do
	local _inverse = {}
	Fruits = setmetatable({}, {
		__index = _inverse,
	})
	Fruits.Apple = 0
	_inverse[0] = "Apple"
	Fruits.Orange = 1
	_inverse[1] = "Orange"
	Fruits.Pear = 2
	_inverse[2] = "Pear"
	Fruits["$"] = 3
	_inverse[3] = "$"
end
-- Numeric enum test
local Breads
do
	local _inverse = {}
	Breads = setmetatable({}, {
		__index = _inverse,
	})
	Breads.White = 5
	_inverse[5] = "White"
	Breads.Wheat = 6
	_inverse[6] = "Wheat"
	Breads.Pumpernickel = 7
	_inverse[7] = "Pumpernickel"
	Breads["$"] = 8
	_inverse[8] = "$"
end
-- String enums
local Soups = {
	Tomato = "TOMATO",
	ChickenNoodle = "CHICKENNOODLE",
	Dumpling = "DUMPLING",
	["$"] = "DOLLARS",
}
local EnumWithEscapedQuote = {
	Quote = '"',
}
local function getValue()
	return 123
end
local WithInitializer
do
	local _inverse = {}
	WithInitializer = setmetatable({}, {
		__index = _inverse,
	})
	local _value = getValue()
	WithInitializer.value = _value
	_inverse[_value] = "value"
end
-- hand-written: mixed values, references between members, negative and fractional values, quoted names
local Mixed
do
	local _inverse = {}
	Mixed = setmetatable({}, {
		__index = _inverse,
	})
	Mixed.A = 1
	_inverse[1] = "A"
	Mixed.B = 2
	_inverse[2] = "B"
	Mixed.C = "c"
	Mixed.D = -3
	_inverse[-3] = "D"
	Mixed.E = 0.5
	_inverse[0.5] = "E"
	Mixed["quoted name"] = 7
	_inverse[7] = "quoted name"
	Mixed.F = 2
	_inverse[2] = "F"
	Mixed.G = 14
	_inverse[14] = "G"
	Mixed.H = -2
	_inverse[-2] = "H"
end
local Exported
do
	local _inverse = {}
	Exported = setmetatable({}, {
		__index = _inverse,
	})
	Exported.One = 1
	_inverse[1] = "One"
	Exported.Two = 2
	_inverse[2] = "Two"
end
local Later
local function readLater()
	return Later.First + Later.Second
end
do
	local _inverse = {}
	Later = setmetatable({}, {
		__index = _inverse,
	})
	Later.First = 10
	_inverse[10] = "First"
	Later.Second = 11
	_inverse[11] = "Second"
end
local function useConstNumbers()
	return 1 + 2
end
local function useConstStrings()
	return "DOG" .. "hidden" .. '"'
end
local function run()
	it("should expose enums by number", function()
		expect(Fruits[0]).to.equal("Apple")
		expect(Fruits.Orange).to.equal(1)
		expect(Fruits[2]).to.equal("Pear")
	end)
	it("should support overriding indices", function()
		expect(Breads[5]).to.equal("White")
		expect(Breads.Wheat).to.equal(6)
		expect(Breads[0]).never.to.be.ok()
	end)
	it("should support for string indices", function()
		expect(Soups.Tomato).to.equal("TOMATO")
		expect(Soups.ChickenNoodle).to.equal("CHICKENNOODLE")
		expect(Soups.Dumpling).to.equal("DUMPLING")
		expect(Soups["$"]).to.equal("DOLLARS")
	end)
	it("should support numeric const enums", function()
		expect(0).to.equal(0)
		expect(1).to.equal(1)
		expect(1).to.equal(1)
		expect(5).to.equal(5)
	end)
	it("should support string const enums", function()
		expect("BEAR").to.equal("BEAR")
		expect("DOG").to.equal("DOG")
		expect("SCARAB").to.equal("SCARAB")
	end)
	it("should support members with initializers", function()
		expect(WithInitializer.value).to.equal(123)
	end)
	local Animal2
	it("should support hoisted enums", function()
		expect(Animal2.Bear).to.equal("BEAR")
		expect(Animal2.Dog).to.equal("DOG")
		expect(Animal2.Snake).to.equal("SNAKE")
		expect(readLater()).to.equal(21)
	end)
	Animal2 = {
		Bear = "BEAR",
		Dog = "DOG",
		Snake = "SNAKE",
	}
	it("should support enums with escaped quotes", function()
		expect(EnumWithEscapedQuote.Quote).to.equal('"')
	end)
	it("should support const enums with escaped quotes", function()
		expect('"').to.equal('"')
	end)
	it("should support mixed enums", function()
		expect(Mixed.B).to.equal(2)
		expect(Mixed[Mixed.D]).to.equal("D")
		expect(Mixed["quoted name"]).to.equal(7)
		expect(Exported[Exported.Two]).to.equal("Two")
	end)
	it("should support computed keys", function()
		local constKeyIndex = 4
		local constKeyValue = 7
		local ConstComputedKey
		do
			local _inverse = {}
			ConstComputedKey = setmetatable({}, {
				__index = _inverse,
			})
			ConstComputedKey[constKeyIndex] = constKeyValue
			_inverse[constKeyValue] = constKeyIndex
		end
		expect(ConstComputedKey[4]).to.equal(7)
		local computedKeyIndex = 1
		local ComputedKey
		do
			local _inverse = {}
			ComputedKey = setmetatable({}, {
				__index = _inverse,
			})
			local _original = computedKeyIndex
			computedKeyIndex += 1
			ComputedKey[_original] = computedKeyIndex
			_inverse[computedKeyIndex] = _original
		end
		expect(ComputedKey[1]).to.equal(2)
		expect(computedKeyIndex).to.equal(2)
		local computedValueIndex = 3
		local PrereqInValue
		do
			local _inverse = {}
			PrereqInValue = setmetatable({}, {
				__index = _inverse,
			})
			local _computedValueIndex = computedValueIndex
			computedValueIndex = 6
			PrereqInValue[_computedValueIndex] = computedValueIndex
			_inverse[computedValueIndex] = _computedValueIndex
		end
		expect(PrereqInValue[3]).to.equal(6)
		expect(computedValueIndex).to.equal(6)
		local StringKey
		do
			local _inverse = {}
			StringKey = setmetatable({}, {
				__index = _inverse,
			})
			StringKey["literal key"] = 1
			_inverse[1] = "literal key"
			StringKey["other key"] = "two"
		end
		expect(StringKey["literal key"]).to.equal(1)
	end)
end
return {
	Exported = Exported,
	useConstNumbers = useConstNumbers,
	useConstStrings = useConstStrings,
	run = run,
}
