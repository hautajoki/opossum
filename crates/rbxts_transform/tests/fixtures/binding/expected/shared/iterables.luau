-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local function values()
	return TS.generator(function()
		coroutine.yield(1)
		coroutine.yield(2)
		coroutine.yield(3)
	end)
end
local function strings()
	local str = "héllo"
	local _matcher = string.gmatch(str, utf8.charpattern)
	local ch0 = _matcher()
	local ch1 = _matcher()
	local _rest = {}
	for _char in _matcher do
		table.insert(_rest, _char)
	end
	local chRest = _rest
	local _matcher_1 = string.gmatch(str, utf8.charpattern)
	_matcher_1()
	local second = _matcher_1()
	local c = ""
	local cs = {}
	local _binding = str
	local _matcher_2 = string.gmatch(_binding, utf8.charpattern)
	c = _matcher_2()
	local _rest_1 = {}
	for _char in _matcher_2 do
		table.insert(_rest_1, _char)
	end
	cs = _rest_1
	return { ch0, ch1, chRest, second, c, cs }
end
local function collections()
	local set = {
		[1] = true,
		[2] = true,
		[3] = true,
	}
	local _key
	_key = next(set, _key)
	local _done = _key == nil
	local _consumed = {}
	if _key ~= nil then
		_consumed[_key] = true
	end
	local s0 = _key
	local _value
	if not _done then
		if _key ~= nil and set[_key] == nil then
			_key = nil
		end
		_key = next(set, _key)
		while _key ~= nil and _consumed[_key] do
			_key = next(set, _key)
		end
		_done = _key == nil
		_value = _key
	end
	local s1 = _value
	local _key_1
	_key_1 = next(set, _key_1)
	local _done_1 = _key_1 == nil
	local sOnly = _key_1
	local _key_2
	_key_2 = next(set, _key_2)
	local _done_2 = _key_2 == nil
	local _consumed_1 = {}
	if _key_2 ~= nil then
		_consumed_1[_key_2] = true
	end
	local sFirst = _key_2
	if not _done_2 then
		if _key_2 ~= nil and set[_key_2] == nil then
			_key_2 = nil
		end
		_key_2 = next(set, _key_2)
		while _key_2 ~= nil and _consumed_1[_key_2] do
			_key_2 = next(set, _key_2)
		end
		_done_2 = _key_2 == nil
	end
	if _key_2 ~= nil then
		_consumed_1[_key_2] = true
	end
	local _rest = {}
	if not _done_2 then
		local _length = 0
		while true do
			if _key_2 ~= nil and set[_key_2] == nil then
				_key_2 = nil
			end
			_key_2 = next(set, _key_2)
			while _key_2 ~= nil and _consumed_1[_key_2] do
				_key_2 = next(set, _key_2)
			end
			if _key_2 == nil then
				break
			end
			_length += 1
			_rest[_length] = _key_2
		end
	end
	local sRest = _rest
	local map = {
		a = 1,
		b = 2,
	}
	local _key_3
	local _value_1
	_key_3, _value_1 = next(map, _key_3)
	local _done_3 = _key_3 == nil
	local _consumed_2 = {}
	if _key_3 ~= nil then
		_consumed_2[_key_3] = true
	end
	local _binding = if _done_3 then nil else { _key_3, _value_1 }
	local k0 = _binding[1]
	local v0 = _binding[2]
	local _value_2
	if not _done_3 then
		local _value_3
		if _key_3 ~= nil and map[_key_3] == nil then
			_key_3 = nil
		end
		_key_3, _value_3 = next(map, _key_3)
		while _key_3 ~= nil and _consumed_2[_key_3] do
			_key_3, _value_3 = next(map, _key_3)
		end
		_done_3 = _key_3 == nil
		_value_2 = if _done_3 then nil else { _key_3, _value_3 }
	end
	local entry1 = _value_2
	local _key_4
	_key_4 = next(map, _key_4)
	local _done_4 = _key_4 == nil
	local _consumed_3 = {}
	if _key_4 ~= nil then
		_consumed_3[_key_4] = true
	end
	local _value_3
	if not _done_4 then
		local _value_4
		if _key_4 ~= nil and map[_key_4] == nil then
			_key_4 = nil
		end
		_key_4, _value_4 = next(map, _key_4)
		while _key_4 ~= nil and _consumed_3[_key_4] do
			_key_4, _value_4 = next(map, _key_4)
		end
		_done_4 = _key_4 == nil
		_value_3 = if _done_4 then nil else { _key_4, _value_4 }
	end
	local mapSecond = _value_3
	local assigned = 0
	local _binding_1 = set
	local _key_5
	_key_5 = next(_binding_1, _key_5)
	local _done_5 = _key_5 == nil
	assigned = _key_5
	return { s0, s1, sOnly, sFirst, sRest, k0, v0, entry1, mapSecond, assigned }
end
local function iterators()
	local _binding = string.gmatch("a b", "%a")
	local _value = { _binding() }
	local _done = _value[1] == nil
	local m0 = if _done then nil else _value
	local _value_1
	if not _done then
		local _value_2 = { _binding() }
		_done = _value_2[1] == nil
		_value_1 = if _done then nil else _value_2
	end
	local m1 = _value_1
	local _binding_1 = values()
	local _next = _binding_1.next
	local _result = _next()
	local _done_1 = _result.done
	local g0 = if _done_1 then nil else _result.value
	if not _done_1 then
		local _result_1 = _next()
		_done_1 = _result_1.done
	end
	local _rest = {}
	if not _done_1 then
		local _length = 0
		while true do
			local _result_1 = _next()
			if _result_1.done then
				break
			end
			_length += 1
			_rest[_length] = _result_1.value
		end
	end
	local gRest = _rest
	local source = values()
	local _next_1 = source.next
	local _result_1 = _next_1()
	local _done_2 = _result_1.done
	local i0 = if _done_2 then nil else _result_1.value
	local _value_2
	if not _done_2 then
		local _result_2 = _next_1()
		_done_2 = _result_2.done
		_value_2 = if _done_2 then nil else _result_2.value
	end
	local i1 = _value_2
	if i1 == nil then
		i1 = 7
	end
	local record = {
		a = 1,
	}
	local _binding_2 = pairs(record)
	local _value_3 = { _binding_2() }
	local _done_3 = _value_3[1] == nil
	local _binding_3 = if _done_3 then nil else _value_3
	local pk = _binding_3[1]
	local pv = _binding_3[2]
	local _binding_4 = ipairs({ 1, 2 })
	local _value_4 = { _binding_4() }
	local _done_4 = _value_4[1] == nil
	local ipFirst = if _done_4 then nil else _value_4
	local assigned = 0
	local rest = {}
	local _binding_5 = values()
	local _next_2 = _binding_5.next
	local _result_2 = _next_2()
	local _done_5 = _result_2.done
	assigned = if _done_5 then nil else _result_2.value
	local _rest_1 = {}
	if not _done_5 then
		local _length = 0
		while true do
			local _result_3 = _next_2()
			if _result_3.done then
				break
			end
			_length += 1
			_rest_1[_length] = _result_3.value
		end
	end
	rest = _rest_1
	return { m0, m1, g0, gRest, i0, i1, pk, pv, ipFirst, assigned, rest }
end
local function sharedTables()
	local sharedTable = SharedTable.new()
	sharedTable.a = 1
	local a = sharedTable.a
	local _iterable = sharedTable
	local _iterator = coroutine.wrap(function()
		for _key, _value in _iterable do
			coroutine.yield(_key, _value)
		end
	end)
	local _value = { _iterator() }
	local _done = _value[1] == nil
	local first = if _done then nil else _value
	local _rest = {}
	if not _done then
		local _length = 0
		while true do
			local _value_1 = { _iterator() }
			if _value_1[1] == nil then
				break
			end
			_length += 1
			_rest[_length] = _value_1
		end
	end
	local others = _rest
	return { a, first, others }
end
local function nestedRest()
	local value = 0
	local tail = {}
	local _binding = {
		[1] = true,
	}
	local _key
	local _rest = {}
	local _length = 0
	while true do
		_key = next(_binding, _key)
		if _key == nil then
			break
		end
		_length += 1
		_rest[_length] = _key
	end
	local _binding_1 = _rest
	value = _binding_1[1]
	tail = table.move(_binding_1, 2, #_binding_1, 1, {})
	local key = ""
	local _binding_2 = {
		key = 2,
	}
	local _key_1
	local _rest_1 = {}
	local _length_1 = 0
	while true do
		local _value
		_key_1, _value = next(_binding_2, _key_1)
		if _key_1 == nil then
			break
		end
		_length_1 += 1
		_rest_1[_length_1] = { _key_1, _value }
	end
	local _binding_3 = _rest_1
	local _binding_4 = _binding_3[1]
	key = _binding_4[1]
	value = _binding_4[2]
	local _binding_5 = values()
	local _next = _binding_5.next
	local _rest_2 = {}
	local _length_2 = 0
	while true do
		local _result = _next()
		if _result.done then
			break
		end
		_length_2 += 1
		_rest_2[_length_2] = _result.value
	end
	local _binding_6 = _rest_2
	value = _binding_6[1]
	local _binding_7 = "🍓a"
	local _matcher = string.gmatch(_binding_7, utf8.charpattern)
	local _rest_3 = {}
	for _char in _matcher do
		table.insert(_rest_3, _char)
	end
	local _binding_8 = _rest_3
	key = _binding_8[1]
	local _binding_9 = string.gmatch("a,b", "[^,]+")
	local _rest_4 = {}
	local _length_3 = 0
	while true do
		local _value = { _binding_9() }
		if _value[1] == nil then
			break
		end
		_length_3 += 1
		_rest_4[_length_3] = _value
	end
	local _binding_10 = _rest_4
	local _binding_11 = _binding_10[1]
	key = _binding_11[1]
	return { value, tail, key }
end
return {
	values = values,
	strings = strings,
	collections = collections,
	iterators = iterators,
	sharedTables = sharedTables,
	nestedRest = nestedRest,
}
