-- Compiled with roblox-ts v3.0.0
-- probe: before the statement
print("before 1")
-- a root statement's leading comment: insert_before_statement goes above it
local b1 = 1
local function f(x)
	-- probe: before the statement
	print("before x + 1")
	-- inside a function body
	local y = x + 1
	if x > 0 then
		-- probe: before the statement
		print("before y")
		return y
	else
		-- probe: before the statement
		print("before x")
		if x < -1 then
			print("negative")
		end
	end
	-- probe: before the statement
	print("before x")
	while x > 10 do
		x -= 1
	end
	repeat
		if x == 1 then
			-- probe: before the statement
			print('before "case"')
			print("case")
			break
		end
	until true
	-- probe: before the statement
	print("before 3")
	local arrow = function()
		return 3
	end
	return arrow
end
-- probe: before the statement
print("before 4")
local Fields
do
	Fields = setmetatable({}, {
		__tostring = function()
			return "Fields"
		end,
	})
	Fields.__index = Fields
	function Fields.new(...)
		local self = setmetatable({}, Fields)
		return self:constructor(...) or self
	end
	function Fields:constructor()
		self.field = 4
	end
end
-- probe: prereq
-- a plugin comment leading its statement's Luau goes above the statement's leading comment
local _probed = 5
local p1 = _probed
-- several insertions before one statement go in TypeScript's visit order (source order), not roblox-ts's lowering
-- order: a `for` head's condition before its incrementor, a class's members as written (a static field before a
-- method's parameter initializer and a later instance field)
local function loop()
	-- probe: before the statement
	print("before 0")
	-- probe: before the statement
	print("before 3")
	-- probe: before the statement
	print("before 1")
	do
		local i = 0
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				i += 1
			else
				_shouldIncrement = true
			end
			if not (i < 3) then
				break
			end
			print(i)
		end
	end
end
-- probe: before the statement
print('before "static first"')
-- probe: before the statement
print('before "parameter"')
-- probe: before the statement
print('before "instance second"')
-- probe: before the statement
print('before "static third"')
local Ordered
do
	Ordered = setmetatable({}, {
		__tostring = function()
			return "Ordered"
		end,
	})
	Ordered.__index = Ordered
	function Ordered.new(...)
		local self = setmetatable({}, Ordered)
		return self:constructor(...) or self
	end
	function Ordered:constructor()
		self.second = "instance second"
	end
	function Ordered:method(a)
		if a == nil then
			a = "parameter"
		end
		return a
	end
	Ordered.first = "static first"
	Ordered.third = "static third"
end
return {
	b1 = b1,
	f = f,
	Fields = Fields,
	p1 = p1,
	loop = loop,
	Ordered = Ordered,
}
