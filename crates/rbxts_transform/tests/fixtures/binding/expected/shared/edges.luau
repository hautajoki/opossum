-- Compiled with roblox-ts v3.0.0
local function strings(text, index)
	local c0 = if 0 < #text then string.sub(text, 1, 1) else nil
	local _key = index
	local cAt = if _key >= 0 and _key % 1 == 0 and _key < #text then string.sub(text, _key + 1, _key + 1) else nil
	local c1 = if 1 < #text then string.sub(text, 2, 2) else nil
	local _binding = "abc"
	local c2 = if 2 < #_binding then string.sub(_binding, 3, 3) else nil
	return { c0, cAt, c1, c2 }
end
local function arrays(list, index)
	local first = list[1]
	local _key = index
	local atIndex = list[_key + 1]
	local _key_1 = index + 1
	local following = list[_key_1 + 1]
	local second = list[2]
	local _rest = {}
	for _key_2, _value in list do
		if _key_2 ~= 2 then
			_rest[_key_2] = _value
		end
	end
	local others = _rest
	return { first, atIndex, following, second, others }
end
local function maps(map, readonly)
	local _key
	local _value
	_key, _value = next(map, _key)
	local _done = _key == nil
	local a = if _done then nil else { _key, _value }
	local _key_1
	local _value_1
	_key_1, _value_1 = next(readonly, _key_1)
	local _done_1 = _key_1 == nil
	local _consumed = {}
	if _key_1 ~= nil then
		_consumed[_key_1] = true
	end
	local b = if _done_1 then nil else { _key_1, _value_1 }
	local _value_2
	if not _done_1 then
		local _value_3
		if _key_1 ~= nil and readonly[_key_1] == nil then
			_key_1 = nil
		end
		_key_1, _value_3 = next(readonly, _key_1)
		while _key_1 ~= nil and _consumed[_key_1] do
			_key_1, _value_3 = next(readonly, _key_1)
		end
		_done_1 = _key_1 == nil
		_value_2 = if _done_1 then nil else { _key_1, _value_3 }
	end
	local c = _value_2
	local d
	local _binding = readonly
	local _key_2
	local _value_3
	_key_2, _value_3 = next(_binding, _key_2)
	local _done_2 = _key_2 == nil
	d = if _done_2 then nil else { _key_2, _value_3 }
	return { a, b, c, d }
end
local function tuples(tuple)
	local n = tuple[1]
	local s = tuple[2]
	local flags = table.move(tuple, 3, #tuple, 1, {})
	local x = 0
	local rest = {}
	local _binding = tuple
	x = _binding[1]
	rest = table.move(_binding, 2, #_binding, 1, {})
	local flag
	local _binding_1 = tuple
	local _binding_2 = table.move(_binding_1, 3, #_binding_1, 1, {})
	flag = _binding_2[1]
	return { n, s, flags, x, rest, flag }
end
local function defaults(obj)
	local a = obj.a
	if a == nil then
		a = 1
	end
	local _binding = obj.b
	if _binding == nil then
		_binding = {}
	end
	local c = _binding.c
	if c == nil then
		c = "c"
	end
	local z = 0
	local w = ""
	local _binding_1 = obj
	local _binding_2 = _binding_1.a
	if _binding_2 == nil then
		_binding_2 = 3
	end
	z = _binding_2
	local _binding_3 = _binding_1.b
	if _binding_3 == nil then
		_binding_3 = {}
	end
	local _binding_4 = _binding_3.c
	if _binding_4 == nil then
		_binding_4 = "w"
	end
	w = _binding_4
	return { a, c, z, w }
end
local function sideEffects()
	local order = {}
	local log = function(n)
		table.insert(order, n)
		return n
	end
	local target = { 0, 0, 0 }
	local _binding = log(2)
	local _binding_1 = log(3)
	local _index = log(0) + 1
	local _index_1 = log(1) + 1
	target[_index], target[_index_1] = _binding, _binding_1
	local p, q = nil, log(6)
	if p == nil then
		p = log(4)
	end
	if q == nil then
		q = log(5)
	end
	return { order, p, q }
end
local Holder
do
	Holder = setmetatable({}, {
		__tostring = function()
			return "Holder"
		end,
	})
	Holder.__index = Holder
	function Holder.new(...)
		local self = setmetatable({}, Holder)
		return self:constructor(...) or self
	end
	function Holder:constructor(items, _param)
		local a = _param.a
		local b = _param.b
		self.items = items
		self.value = 1
		print(a, b)
	end
	function Holder:method(_param, _param_1)
		local first = _param[1]
		local rest = table.move(_param, 2, #_param, 1, {})
		if _param_1 == nil then
			_param_1 = {}
		end
		local x = _param_1.x
		if x == nil then
			x = 2
		end
		local value = self.value
		local _binding = self.items
		self.value = _binding[1]
		local _binding_1 = {
			value = first,
		}
		self.value = _binding_1.value
		return { first, rest, x, value }
	end
	function Holder:make(a, b)
		return Holder.new({ a, b }, {
			a = a,
			b = b,
		})
	end
end
return {
	strings = strings,
	arrays = arrays,
	maps = maps,
	tuples = tuples,
	defaults = defaults,
	sideEffects = sideEffects,
	Holder = Holder,
}
