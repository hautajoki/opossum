-- Compiled with roblox-ts v3.0.0
-- ReadonlySet, Set, ReadonlyMap and Map macros, and the constructor macros
local function sets(set, ro, key)
	local _exp = next(ro) == nil
	-- ▼ ReadonlySet.size ▼
	local _size = 0
	for _ in ro do
		_size += 1
	end
	-- ▲ ReadonlySet.size ▲
	print(_exp, _size, ro[key] ~= nil, set.a ~= nil)
	local _arg0 = function(v, v2, s)
		return print(v, v2, s)
	end
	for _v in ro do
		_arg0(_v, _v, ro)
	end
	set[key] = true
	set.x = true
	local same = set
	set[key] = nil
	-- ▼ Set.delete ▼
	local _valueExisted = set.y ~= nil
	set.y = nil
	-- ▲ Set.delete ▲
	local existed = _valueExisted
	table.clear(set)
	-- ▼ ReadonlySet.size ▼
	local _size_1 = 0
	for _ in set do
		_size_1 += 1
	end
	-- ▲ ReadonlySet.size ▲
	print(same, existed, _size_1)
end
local function maps(map, ro, getKey)
	local _exp = next(ro) == nil
	-- ▼ ReadonlyMap.size ▼
	local _size = 0
	for _ in ro do
		_size += 1
	end
	-- ▲ ReadonlyMap.size ▲
	print(_exp, _size, ro.a ~= nil, ro.a, ro[getKey()])
	local _arg0 = function(v, k, m)
		return print(v, k, m)
	end
	for _k, _v in ro do
		_arg0(_v, _k, ro)
	end
	map.a = 1
	local _arg0_1 = getKey()
	-- ▼ ReadonlyMap.size ▼
	local _size_1 = 0
	for _ in map do
		_size_1 += 1
	end
	-- ▲ ReadonlyMap.size ▲
	map[_arg0_1] = _size_1
	map.b = 2
	local returned = map
	map.a = nil
	local _arg0_2 = getKey()
	-- ▼ Map.delete ▼
	local _valueExisted = map[_arg0_2] ~= nil
	map[_arg0_2] = nil
	-- ▲ Map.delete ▲
	local existed = _valueExisted
	local value = map.b
	if value ~= nil then
		print(value + 1)
	end
	table.clear(map)
	print(returned, existed, map.c)
end
local function order(getMap, k, v)
	getMap()[k()] = v()
	print(getMap()[k()])
	local has = getMap()[k()] ~= nil
	local _exp = getMap()
	local _arg0 = k()
	-- ▼ Map.delete ▼
	local _valueExisted = _exp[_arg0] ~= nil
	_exp[_arg0] = nil
	-- ▲ Map.delete ▲
	local removed = _valueExisted
	return has and removed
end
local function constructors(arr, entries, instances, n)
	local a = {}
	local b = table.create(n)
	local c = {}
	local _set = {}
	for _, _v in arr do
		_set[_v] = true
	end
	local d = _set
	local e = {}
	local _map = {}
	for _, _v in entries do
		_map[_v[1]] = _v[2]
	end
	local f = _map
	local g = setmetatable({}, {
		__mode = "k",
	})
	local h = setmetatable({}, {
		__mode = "k",
	})
	local _set_1 = {}
	for _, _v in arr do
		_set_1[_v] = true
	end
	local i = _set_1
	local _map_1 = {}
	for _, _v in entries do
		_map_1[_v[1]] = _v[2]
	end
	local j = _map_1
	local _arg0 = function(entry)
		return entry
	end
	-- ▼ ReadonlyArray.map ▼
	local _newValue = table.create(#instances)
	for _k, _v in instances do
		_newValue[_k] = _arg0(_v, _k - 1, instances)
	end
	-- ▲ ReadonlyArray.map ▲
	local _map_2 = {}
	for _, _v in _newValue do
		_map_2[_v[1]] = _v[2]
	end
	local k = setmetatable(_map_2, {
		__mode = "k",
	})
	print(a, b, c, d, e, f, g, h, i, j, k)
end
local function classes(n)
	local part = Instance.new("Part")
	local v = Vector3.new(1, 2, n)
	local zero = Vector3.new()
	local cf = CFrame.new(v)
	print(part, v, zero, cf)
end
return {
	sets = sets,
	maps = maps,
	order = order,
	constructors = constructors,
	classes = classes,
}
