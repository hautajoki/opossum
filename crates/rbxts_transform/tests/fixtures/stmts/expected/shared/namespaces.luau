-- Compiled with roblox-ts v3.0.0
-- Namespaces: roblox-ts tests' namespace.spec.ts (without its object literals), plus hand-written cases.
local n1
local function readNamespace()
	return n1.a
end
n1 = {}
do
	local _container = n1
	local a = "a"
	_container.a = a
	local function read(value)
		return value
	end
	_container.read = read
	local Shape = 42
	_container.Shape = Shape
	local Label = "label"
	_container.Label = Label
end
local dotted = {}
do
	local _container = dotted
	local inner = {}
	do
		local _container_1 = inner
		local value = 42
		_container_1.value = value
	end
	_container.inner = inner
end
local n2 = {}
do
	local _container = n2
	local n3 = {}
	do
		local _container_1 = n3
		local a = "a"
		_container_1.a = a
	end
	_container.n3 = n3
end
local foo = {}
do
	local _container = foo
	local foo = {}
	do
		local _container_1 = foo
		local foo = {}
		do
			local _container_2 = foo
			local bar = "bar"
			_container_2.bar = bar
		end
		_container_1.foo = foo
	end
	_container.foo = foo
end
local namespaceEffects = 0
local Empty = {}
do
	namespaceEffects += 1
end
local WithFunction = {}
do
	local _container = WithFunction
	local function value()
		return 42
	end
	_container.value = value
end
local Nested = {}
do
	local _container = Nested
	local Self = {}
	do
		local _container_1 = Self
		local function value()
			return Self
		end
		_container_1.value = value
	end
	_container.Self = Self
end
local EnumBeforeNamespace
do
	local _inverse = {}
	EnumBeforeNamespace = setmetatable({}, {
		__index = _inverse,
	})
	EnumBeforeNamespace.Value = 3
	_inverse[3] = "Value"
end
local NamespaceBeforeEnum
do
	local _inverse = {}
	NamespaceBeforeEnum = setmetatable({}, {
		__index = _inverse,
	})
	NamespaceBeforeEnum.Value = 4
	_inverse[4] = "Value"
end
-- hand-written: mutable exports, enums and private members inside, exported and local namespaces
local Counter = {}
do
	local _container = Counter
	_container.count = 0
	local step = 1
	local function increment()
		_container.count += step
		return _container.count
	end
	_container.increment = increment
	local Kind
	do
		local _inverse = {}
		Kind = setmetatable({}, {
			__index = _inverse,
		})
		Kind.Up = 0
		_inverse[0] = "Up"
		Kind.Down = 1
		_inverse[1] = "Down"
	end
	_container.Kind = Kind
	local hiddenValue = 1
	_container.hiddenValue = hiddenValue
end
local Public = {}
do
	local _container = Public
	local name = "public"
	_container.name = name
	local function greet()
		return name
	end
	_container.greet = greet
	local Inner = {}
	do
		local _container_1 = Inner
		local depth = 2
		_container_1.depth = depth
	end
	_container.Inner = Inner
end
local Outer = {}
do
	local _container = Outer
	local Local = {}
	do
		local _container_1 = Local
		local value = 1 + Ambient.value
		_container_1.value = value
	end
	local function makeLocal()
		return Local.value
	end
	_container.makeLocal = makeLocal
end
local function makeLocal()
	return Outer.makeLocal()
end
local function run()
	it("should preserve enums merged with namespaces containing only types", function()
		local label = NamespaceBeforeEnum[NamespaceBeforeEnum.Value]
		expect(EnumBeforeNamespace.Value).to.equal(3)
		expect(EnumBeforeNamespace[3]).to.equal("Value")
		expect(label).to.equal("Value")
	end)
	it("should execute namespaces without exports", function()
		expect(namespaceEffects).to.equal(1)
		expect(WithFunction.value()).to.equal(42)
		expect(Nested.Self.value()).to.equal(Nested.Self)
	end)
	it("should support namespaces", function()
		expect(n1.a).to.equal("a")
		expect(readNamespace()).to.equal("a")
		expect(n1.read("value")).to.equal("value")
		expect(n1.Shape).to.equal(42)
		expect(n1.Label).to.equal("label")
		expect(dotted.inner.value).to.equal(42)
	end)
	it("should support nested namespaces", function()
		expect(n2.n3.a).to.equal("a")
	end)
	it("should support shadowed namespaces", function()
		expect(foo.foo.foo.bar).to.equal("bar")
	end)
	it("should support mutable namespace exports", function()
		expect(Counter.increment()).to.equal(1)
		expect(Counter.count).to.equal(1)
		Counter.count = 5
		expect(Counter.Kind.Down).to.equal(1)
		expect(Counter.hiddenValue).to.equal(1)
		expect(Public.greet()).to.equal(Public.name)
		expect(Public.Inner.depth).to.equal(2)
	end)
end
return {
	Public = Public,
	makeLocal = makeLocal,
	run = run,
}
