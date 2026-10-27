-- Compiled with roblox-ts v3.0.0
-- ReadonlyArray and Array macros, used as values and as statements
local function readonlyArrays(arr, strs, parts, maybeSep)
	print(#arr, #arr == 0, #strs == 0)
	local _exp = table.concat(strs, ", ")
	local _exp_1 = table.concat(strs, ", ")
	-- ▼ ReadonlyArray.join ▼
	local _result = table.create(#parts)
	for _k, _v in parts do
		_result[_k] = tostring(_v)
	end
	-- ▲ ReadonlyArray.join ▲
	print(_exp, _exp_1, table.concat(_result, "-"), table.concat(strs, if maybeSep == nil then ", " else maybeSep), table.concat(strs, ", "))
	print(table.find(arr, 1) ~= nil, table.find(arr, 2, 4) ~= nil, (table.find(arr, 4) or 0) - 1, (table.find(arr, 5, 2) or 0) - 1)
	local _arg0 = function(v)
		return v > 0
	end
	-- ▼ ReadonlyArray.every ▼
	local _result_1 = true
	for _k, _v in arr do
		if not _arg0(_v, _k - 1, arr) then
			_result_1 = false
			break
		end
	end
	-- ▲ ReadonlyArray.every ▲
	local _arg0_1 = function(v, i)
		return v == i
	end
	-- ▼ ReadonlyArray.some ▼
	local _result_2 = false
	for _k, _v in arr do
		if _arg0_1(_v, _k - 1, arr) then
			_result_2 = true
			break
		end
	end
	-- ▲ ReadonlyArray.some ▲
	print(_result_1, _result_2)
	local _arg0_2 = function(v, i, a)
		return print(v, i, a)
	end
	for _k, _v in arr do
		_arg0_2(_v, _k - 1, arr)
	end
	local _arg0_3 = function(v)
		return print(v)
	end
	for _k, _v in arr do
		_arg0_3(_v, _k - 1, arr)
	end
	local each = nil
	local _arg0_4 = function(v)
		return v * 2
	end
	-- ▼ ReadonlyArray.map ▼
	local _newValue = table.create(#arr)
	for _k, _v in arr do
		_newValue[_k] = _arg0_4(_v, _k - 1, arr)
	end
	-- ▲ ReadonlyArray.map ▲
	local doubled = _newValue
	local _arg0_5 = function(p)
		return p.Parent
	end
	-- ▼ ReadonlyArray.mapFiltered ▼
	local _newValue_1 = {}
	local _length = 0
	for _k, _v in parts do
		local _result_3 = _arg0_5(_v, _k - 1, parts)
		if _result_3 ~= nil then
			_length += 1
			_newValue_1[_length] = _result_3
		end
	end
	-- ▲ ReadonlyArray.mapFiltered ▲
	local parents = _newValue_1
	-- ▼ ReadonlyArray.filterUndefined ▼
	local _length_1 = 0
	for _i in arr do
		if _i > _length_1 then
			_length_1 = _i
		end
	end
	local _result_3 = {}
	local _resultLength = 0
	for _i = 1, _length_1 do
		local _v = arr[_i]
		if _v ~= nil then
			_resultLength += 1
			_result_3[_resultLength] = _v
		end
	end
	-- ▲ ReadonlyArray.filterUndefined ▲
	local defined = _result_3
	local _arg0_6 = function(v)
		return v > 10
	end
	-- ▼ ReadonlyArray.filter ▼
	local _newValue_2 = {}
	local _length_2 = 0
	for _k, _v in arr do
		if _arg0_6(_v, _k - 1, arr) == true then
			_length_2 += 1
			_newValue_2[_length_2] = _v
		end
	end
	-- ▲ ReadonlyArray.filter ▲
	local big = _newValue_2
	local _arg0_7 = function(acc, v)
		return acc + v
	end
	-- ▼ ReadonlyArray.reduce ▼
	if #arr == 0 then
		error("Attempted to call `ReadonlyArray.reduce()` on an empty array without an initialValue.")
	end
	local _result_4 = arr[1]
	for _i = 2, #arr do
		_result_4 = _arg0_7(_result_4, arr[_i], _i - 1, arr)
	end
	-- ▲ ReadonlyArray.reduce ▲
	local sum = _result_4
	local _arg0_8 = function(acc, v, i)
		return acc + v * i
	end
	-- ▼ ReadonlyArray.reduce ▼
	local _result_5 = 0
	for _i = 1, #arr do
		_result_5 = _arg0_8(_result_5, arr[_i], _i - 1, arr)
	end
	-- ▲ ReadonlyArray.reduce ▲
	local total = _result_5
	local _arg0_9 = function(v)
		return v == 3
	end
	-- ▼ ReadonlyArray.find ▼
	local _result_6
	for _i, _v in arr do
		if _arg0_9(_v, _i - 1, arr) == true then
			_result_6 = _v
			break
		end
	end
	-- ▲ ReadonlyArray.find ▲
	local found = _result_6
	local _arg0_10 = function(v)
		return v == 3
	end
	-- ▼ ReadonlyArray.findIndex ▼
	local _result_7 = -1
	for _i, _v in arr do
		if _arg0_10(_v, _i - 1, arr) == true then
			_result_7 = _i - 1
			break
		end
	end
	-- ▲ ReadonlyArray.findIndex ▲
	local foundIndex = _result_7
	print(each, doubled, parents, defined, big, sum, total, found, foundIndex)
	table.move(arr, 1, 2, 3)
	return table.move(arr, 1, 2, 3, arr)
end
local function arrayMutators(arr, getArr, f)
	local _ = #arr
	table.insert(arr, 1)
	local _arg0 = f()
	local _arg2 = f()
	-- ▼ Array.push ▼
	table.insert(arr, _arg0)
	table.insert(arr, 2)
	table.insert(arr, _arg2)
	-- ▲ Array.push ▲
	table.insert(arr, 3)
	local len = #arr
	table.insert(getArr(), 4)
	-- ▼ Array.pop ▼
	local _length = #arr
	local _result = arr[_length]
	arr[_length] = nil
	-- ▲ Array.pop ▲
	print(_result)
	arr[#arr] = nil
	print(table.remove(arr, 1))
	table.remove(arr, 1)
	-- ▼ Array.unshift ▼
	table.insert(arr, 1, 6)
	table.insert(arr, 1, 5)
	-- ▲ Array.unshift ▲
	table.insert(arr, 1, 7)
	local len2 = #arr
	table.insert(arr, 1, 8)
	table.insert(arr, f() + 1, 9)
	print(table.remove(arr, 1), table.remove(arr, f() + 1))
	-- ▼ Array.unorderedRemove ▼
	local _length_1 = #arr
	local _value = arr[2]
	if _value ~= nil then
		arr[2] = arr[_length_1]
		arr[_length_1] = nil
	end
	-- ▲ Array.unorderedRemove ▲
	-- ▼ Array.unorderedRemove ▼
	local _index = f() + 1
	local _length_2 = #arr
	local _value_1 = arr[_index]
	if _value_1 ~= nil then
		arr[_index] = arr[_length_2]
		arr[_length_2] = nil
	end
	-- ▲ Array.unorderedRemove ▲
	local removed = _value_1
	table.sort(arr)
	table.sort(arr, function(a, b)
		return a < b
	end)
	table.sort(arr)
	local sorted = arr
	table.clear(arr)
	table.clear(arr)
	print(len, len2, removed, sorted, nil)
	table.insert(arr, 10)
	local _exp = #arr
	local _exp_1 = getArr()
	-- ▼ Array.pop ▼
	local _length_3 = #_exp_1
	local _result_1 = _exp_1[_length_3]
	_exp_1[_length_3] = nil
	-- ▲ Array.pop ▲
	print(_exp, _result_1)
end
local function receivers(getArr, f)
	print(#getArr(), table.find(getArr(), f()) ~= nil)
	local _exp = getArr()
	local _arg0 = function(v)
		return print(v)
	end
	for _k, _v in _exp do
		_arg0(_v, _k - 1, _exp)
	end
	local _exp_1 = getArr()
	local _arg0_1 = function(v)
		return v + f()
	end
	-- ▼ ReadonlyArray.map ▼
	local _newValue = table.create(#_exp_1)
	for _k, _v in _exp_1 do
		_newValue[_k] = _arg0_1(_v, _k - 1, _exp_1)
	end
	-- ▲ ReadonlyArray.map ▲
	local mapped = _newValue
	local _exp_2 = getArr()
	-- ▼ Array.unorderedRemove ▼
	local _index = f() + 1
	local _length = #_exp_2
	local _value = _exp_2[_index]
	if _value ~= nil then
		_exp_2[_index] = _exp_2[_length]
		_exp_2[_length] = nil
	end
	-- ▲ Array.unorderedRemove ▲
	return mapped
end
return {
	readonlyArrays = readonlyArrays,
	arrayMutators = arrayMutators,
	receivers = receivers,
}
