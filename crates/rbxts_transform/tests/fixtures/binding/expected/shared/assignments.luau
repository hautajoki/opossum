-- Compiled with roblox-ts v3.0.0
local function luaTuple()
	return 1, 2
end
local function getIndex()
	print("index")
	return 0
end
local function swap()
	local a = 1
	local b = 2
	a, b = b, a
	return a + b
end
local function assignments()
	local a = 1
	local b = 2
	local obj = {
		x = 1,
		y = 2,
		list = { 1, 2, 3 },
	}
	local x = 0
	local y = 0
	-- one named property and an identifier target
	x = obj.x
	a = obj.x
	x = obj.x
	if x == nil then
		x = 9
	end
	b = obj.x
	if b == nil then
		b = 8
	end
	-- general object patterns
	local _binding = obj
	x = _binding.x
	y = _binding.y
	local _binding_1 = obj
	a = _binding_1.x
	local _binding_2 = _binding_1.y
	if _binding_2 == nil then
		_binding_2 = 5
	end
	b = _binding_2
	-- property and element targets
	local target = {
		p = 0,
		q = 0,
	}
	local list = { 0, 0, 0 }
	target.p, target.q = x, y
	local _binding_3 = obj
	target.p = _binding_3.x
	local _binding_4 = obj.list
	list[getIndex() + 1] = _binding_4[1]
	list[2] = _binding_4[2]
	local _binding_5 = obj
	list[getIndex() + 1] = _binding_5.y
	-- rest
	local rest = {}
	local _binding_6 = obj.list
	a = _binding_6[1]
	rest = table.move(_binding_6, 2, #_binding_6, 1, {})
	local objRest
	local _binding_7 = obj
	x = _binding_7.x
	local _rest = {}
	for _key, _value in _binding_7 do
		if _key ~= "x" then
			_rest[_key] = _value
		end
	end
	objRest = _rest
	-- LuaTuple calls
	a, b = luaTuple()
	local _
	_, b = luaTuple()
	local _index = getIndex() + 1
	target.p, list[_index] = luaTuple()
	-- array literals with calls and effects
	a, b = (luaTuple()), b
	local _array = { 1 }
	local _length = #_array
	local _array_1 = obj.list
	table.move(_array_1, 1, #_array_1, _length + 1, _array)
	local _binding_8 = _array
	x = _binding_8[1]
	y = _binding_8[2]
	a = _binding_8[3]
	-- used as expressions
	local _binding_9 = obj.list
	a = _binding_9[1]
	b = _binding_9[2]
	local result = _binding_9
	local _binding_10 = obj
	x = _binding_10.x
	y = _binding_10.y
	local result2 = _binding_10
	local _binding_11 = obj
	x = _binding_11.x
	local result3 = _binding_11
	local empty = (obj.list)
	local emptyObject = obj
	-- nested patterns and defaults
	local n0 = 0
	local n1 = 0
	local _binding_12
	n0, _binding_12 = 1, { 2 }
	n1 = _binding_12[1]
	local _binding_13 = obj.list
	local _binding_14 = _binding_13[1]
	if _binding_14 == nil then
		_binding_14 = 4
	end
	n0 = _binding_14
	n1 = _binding_13[2]
	local _binding_15
	n0, _binding_15 = 1, obj
	n1 = _binding_15.y
	local _binding_16 = obj
	local _binding_17 = _binding_16.list
	n0 = _binding_17[1]
	local _binding_18 = _binding_17[2]
	if _binding_18 == nil then
		_binding_18 = 7
	end
	n1 = _binding_18
	local _binding_19 = obj
	n0 = _binding_19.x
	local _binding_20 = _binding_19.list
	n1 = _binding_20[2]
	-- nested patterns below a rest element
	local source = { 1, { 42 }, { 99 } }
	local _binding_21 = source
	n0 = _binding_21[1]
	local _binding_22 = table.move(_binding_21, 2, #_binding_21, 1, {})
	local _binding_23 = _binding_22[1]
	n1 = _binding_23[1]
	local _binding_24 = obj.list
	a = _binding_24[1]
	local _binding_25 = table.move(_binding_24, 2, #_binding_24, 1, {})
	b = _binding_25[1]
	x = _binding_25[2]
	return { a, b, x, y, rest, objRest, result, result2, result3, empty, emptyObject, n0, n1 }
end
return {
	luaTuple = luaTuple,
	getIndex = getIndex,
	swap = swap,
	assignments = assignments,
}
