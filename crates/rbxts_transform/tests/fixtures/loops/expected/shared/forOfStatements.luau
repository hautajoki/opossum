-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- for-of statements: one loop builder per iterable kind, and $range
local function arrays(values, tuple, readonlyValues)
	for _, v in values do
		print(v)
	end
	for _, v in tuple do
		print(v)
	end
	for _, v in readonlyValues do
		if v then
			continue
		end
		print(v)
	end
	for _, v in getArray() do
		print(v)
	end
	for _, v in { 1, 2, 3 } do
		print(v)
	end
	local last
	for _, _v in values do
		last = _v
		print(last)
	end
	local holder = {
		value = 0,
	}
	for _, _v in values do
		holder.value = _v
		print(holder.value)
	end
	return last
end
local function collections(set, map, shared, weak)
	for v in set do
		print(v)
	end
	for k, v in map do
		print(k, v)
	end
	for _, v in map do
		print(v)
	end
	for k in map do
		print(k)
	end
	for k, v in map do
		if v == nil then
			v = 5
		end
		print(k, v)
	end
	for _k, _v in map do
		local entry = { _k, _v }
		print(entry[1], entry[2])
	end
	for k, v in shared do
		print(k, v)
	end
	for k, v in weak do
		print(k, v)
	end
	local key = ""
	local value = 0
	for _binding, _binding_1 in map do
		key = _binding
		value = _binding_1
		print(key, value)
	end
	for _binding, _binding_1 in map do
		key = _binding
		value = _binding_1
		if value == nil then
			value = 1
		end
		print(key, value)
	end
	local pair
	for _k, _v in map do
		pair = { _k, _v }
		print(pair[1])
	end
	return { key, value }
end
local function strings(text)
	for char in string.gmatch(text, utf8.charpattern) do
		print(char)
	end
	for char in string.gmatch("abc", utf8.charpattern) do
		print(char)
	end
end
local function iterableFunctions(instance, values)
	for k, v in pairs(values) do
		print(k, v)
	end
	for _element, _element_1 in pairs(values) do
		local pair = { _element, _element_1 }
		print(pair[1], pair[2])
	end
	for i, child in ipairs(instance:GetChildren()) do
		print(i, child)
	end
	for _key, _value, _element in labeledIterator() do
		local entry = { _key, _value, _element }
		print(entry[1], entry[2], entry[3])
	end
	local _iterFunc = variadicIterator()
	while true do
		local tuple = { _iterFunc() }
		if tuple[1] == nil then
			break
		end
		print(tuple[1])
	end
	local assigned
	local _iterFunc_1 = variadicIterator()
	while true do
		local _v = { _iterFunc_1() }
		if _v[1] == nil then
			break
		end
		assigned = _v
		print(assigned[1])
	end
	for n in plainIterator() do
		print(n)
	end
	local _iterFunc_2 = string.gmatch("a b", "%a+")
	while true do
		local word = { _iterFunc_2() }
		if word[1] == nil then
			break
		end
		print(word)
	end
end
local function generator()
	return TS.generator(function()
		coroutine.yield(1)
		coroutine.yield(2)
	end)
end
local function generators()
	for _result in generator().next do
		if _result.done then
			break
		end
		local n = _result.value
		print(n)
	end
	local last = 0
	for _result in generator().next do
		if _result.done then
			break
		end
		last = _result.value
		print(last)
	end
	return last
end
local function ranges(n, step)
	for i = 1, 10 do
		print(i)
	end
	for i = 10, 1, -1 do
		print(i)
	end
	for i = 0, n, 2 do
		print(i)
	end
	for i = 0, n, step or 1 do
		print(i)
	end
	for i = 0, n, -step do
		print(i)
	end
	local i = 0
	for _v = 1, n do
		i = _v
		print(i)
	end
	return i
end
local function tryInLoops(values)
	for _, v in values do
		local _exitType, _returns = TS.try(function()
			if v == 0 then
				return TS.TRY_CONTINUE
			end
			if v == 1 then
				return TS.TRY_BREAK
			end
			print(v)
		end, function(e)
			print(e)
		end)
		if _exitType == TS.TRY_BREAK then
			break
		elseif _exitType then
			continue
		end
	end
	do
		local _i = 0
		local _shouldIncrement = false
		while true do
			local i = _i
			if _shouldIncrement then
				i += 1
			else
				_shouldIncrement = true
			end
			if not (i < #values) then
				break
			end
			local _exitType, _returns = TS.try(function()
				if i == 2 then
					return TS.TRY_CONTINUE
				end
				print(function()
					return i
				end)
			end, nil, function()
				print("done")
			end)
			if _exitType then
				_i = i
				continue
			end
			_i = i
		end
	end
end
return {
	arrays = arrays,
	collections = collections,
	strings = strings,
	iterableFunctions = iterableFunctions,
	generators = generators,
	ranges = ranges,
	tryInLoops = tryInLoops,
}
