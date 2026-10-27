-- Compiled with roblox-ts v3.0.0
-- Hand-written literals fixture: object literals.
local key = "k"
local a = 1
-- function declarations first: TypeScript 5 lists them first among the exports, TypeScript 7 in source order
local function build(x)
	return {
		x = x,
		y = x + 1,
		nested = {
			x = x,
		},
	}
end
local function passes()
	local _object = table.clone(record)
	setmetatable(_object, nil)
	print({
		a = 1,
	}, _object)
end
local plain = {
	a = 1,
	b = "two",
	["c-d"] = true,
	[3] = 4,
	[key] = 5,
	["end"] = nil,
}
local shorthand = {
	a = a,
	key = key,
}
local computed = {
	[getKey()] = 1,
	[`t{a}`] = 2,
	literal = 3,
}
local empty = {}
local keys = {
	[-1] = "negative",
	[1.5] = "fraction",
	[`plain`] = 0,
	valid = 1,
	["not valid"] = 2,
}
local nested = {
	inner = {
		deep = {
			deeper = a,
		},
	},
	list = { 1, 2 },
}
-- statements needed by a value end the inline map
local _object = {
	first = getKey(),
}
local _left = "second"
local _condition = n
if _condition == nil then
	_condition = 2
end
_object[_left] = _condition
_object.third = 3
local withPrereqs = _object
-- and so do those needed by a computed key
local _object_1 = {
	first = 1,
}
local _condition_1 = n
if _condition_1 == nil then
	_condition_1 = "fallback"
end
local _left_1 = _condition_1
_object_1[_left_1] = 2
_object_1.last = 3
local keyPrereqs = _object_1
-- spread first: table.clone
local _object_2 = table.clone(record)
setmetatable(_object_2, nil)
_object_2.z = 1
local spreadFirst = _object_2
-- spread after fields: a loop
local _object_3 = {
	z = 1,
}
for _k, _v in record do
	_object_3[_k] = _v
end
local spreadAfter = _object_3
-- a spread that may be undefined
local _object_4 = {}
if maybe then
	for _k, _v in maybe do
		_object_4[_k] = _v
	end
end
_object_4.b = 2
local spreadMaybe = _object_4
local _object_5 = {
	b = 2,
}
if maybe then
	for _k, _v in maybe do
		_object_5[_k] = _v
	end
end
local spreadMaybeAfter = _object_5
-- spread of an object literal and of a call
local _object_6 = table.clone({
	q = 1,
})
setmetatable(_object_6, nil)
_object_6.r = 2
local spreadLiteral = _object_6
local _object_7 = table.clone(getRecord())
setmetatable(_object_7, nil)
local spreadCall = _object_7
local _object_8 = table.clone(record)
setmetatable(_object_8, nil)
if maybe then
	for _k, _v in maybe do
		_object_8[_k] = _v
	end
end
_object_8.last = true
local twoSpreads = _object_8
-- a computed method name with statements: they go before the object, which stays inline
local _condition_2 = n
if _condition_2 == nil then
	_condition_2 = "fallback"
end
local methodKeyPrereqs = {
	first = 1,
	[_condition_2] = function(self)
		return 2
	end,
	last = 3,
}
return {
	build = build,
	passes = passes,
	plain = plain,
	shorthand = shorthand,
	computed = computed,
	empty = empty,
	keys = keys,
	nested = nested,
	withPrereqs = withPrereqs,
	keyPrereqs = keyPrereqs,
	spreadFirst = spreadFirst,
	spreadAfter = spreadAfter,
	spreadMaybe = spreadMaybe,
	spreadMaybeAfter = spreadMaybeAfter,
	spreadLiteral = spreadLiteral,
	spreadCall = spreadCall,
	twoSpreads = twoSpreads,
	methodKeyPrereqs = methodKeyPrereqs,
}
