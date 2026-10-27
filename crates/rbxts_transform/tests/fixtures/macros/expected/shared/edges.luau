-- Compiled with roblox-ts v3.0.0
-- macros in optional chains, chained macros, macro values as operands, element access calls
local function optional(maybeArr, maybeMap)
	local _result = maybeArr
	if _result ~= nil then
		table.insert(_result, 1)
	end
	local _size = maybeArr
	if _size ~= nil then
		_size = #_size
	end
	local size = _size
	local _value = maybeMap
	if _value ~= nil then
		_value = _value.a
	end
	local value = _value
	local _result_1 = maybeMap
	if _result_1 ~= nil then
		_result_1.b = 2
	end
	print(size, value)
end
local function chained(set, map, arr)
	set.a = true
	set.b = true
	local both = set
	map.a = 1
	map.b = 2
	table.sort(arr)
	local _arg0 = function(v)
		return v + 1
	end
	-- ▼ ReadonlyArray.map ▼
	local _newValue = table.create(#arr)
	for _k, _v in arr do
		_newValue[_k] = _arg0(_v, _k - 1, arr)
	end
	-- ▲ ReadonlyArray.map ▲
	local sorted = _newValue
	local _arg0_1 = function(v)
		return v > 0
	end
	-- ▼ ReadonlyArray.filter ▼
	local _newValue_1 = {}
	local _length = 0
	for _k, _v in arr do
		if _arg0_1(_v, _k - 1, arr) == true then
			_length += 1
			_newValue_1[_length] = _v
		end
	end
	-- ▲ ReadonlyArray.filter ▲
	print(both, sorted, #_newValue_1)
end
local function operands(arr, getArr, f, t)
	table.insert(getArr(), f())
	table.insert(arr, #arr)
	table.insert(arr, #arr + 1, f())
	if table.find(arr, f()) ~= nil then
		print(typeof(f()) == t)
	end
	print(table.find(arr, f()) ~= nil, (table.find(arr, 1) or 0) - 1)
	local n = table.create(f())
	return #n + #getArr()
end
return {
	optional = optional,
	chained = chained,
	operands = operands,
}
