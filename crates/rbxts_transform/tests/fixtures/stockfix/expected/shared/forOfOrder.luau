-- Compiled with roblox-ts v3.0.0
-- for-of evaluation order. roblox-ts evaluated a target's prerequisites once, before the loop and before the
-- iterable; applied a nested pattern's default after destructuring the value; and read the iterable before its own
-- prerequisites when the loop builder captured it.
local n = 0
local log = {}
local function key()
	table.insert(log, `key{n}`)
	local _original = n
	n += 1
	return _original
end
local function list()
	table.insert(log, "list")
	return { 10, 20 }
end
-- the target is evaluated on every iteration, after the iterable
local function writableTarget()
	n = 0
	log = {}
	local obj = {}
	for _, _v in list() do
		local _exp = key()
		local _original = n
		n += 1
		obj[_exp + _original] = _v
		table.insert(log, "body")
	end
	return { table.concat(log, " "), obj[1], obj[5] }
end
-- the same through a map loop's pair
local function map()
	table.insert(log, "map")
	return {
		a = 1,
	}
end
local function writableMapTarget()
	n = 0
	log = {}
	local obj = {}
	for _k, _v in map() do
		local _exp = key() * 10
		local _original = n
		n += 1
		obj[_exp + _original] = { _k, _v }
		table.insert(log, "body")
	end
	return { table.concat(log, " "), obj[1][1], obj[1][2] }
end
-- the default applies before the nested pattern reads the value
local function nestedDefault()
	local out = {}
	for k, _binding in entries() do
		if _binding == nil then
			_binding = { 7 }
		end
		local v = _binding[1]
		-- ▼ Array.push ▼
		table.insert(out, k)
		table.insert(out, v)
		-- ▲ Array.push ▲
	end
	return out
end
local box = {
	count = 2,
}
-- the iterable's own prerequisites come before the builder captures it
local function iterablePrerequisites()
	local out = {}
	local _result = box
	if _result ~= nil then
		_result = _result.count
	end
	local _iterFunc = makeIter(_result)
	while true do
		local t = { _iterFunc() }
		if t[1] == nil then
			break
		end
		table.insert(out, t[1])
	end
	return out
end
return {
	writableTarget = writableTarget,
	writableMapTarget = writableMapTarget,
	nestedDefault = nestedDefault,
	iterablePrerequisites = iterablePrerequisites,
}
