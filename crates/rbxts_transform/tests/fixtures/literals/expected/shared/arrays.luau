-- Compiled with roblox-ts v3.0.0
-- Hand-written literals fixture: array literals and array spread.
local simple = { 1, 2, 3 }
local empty = {}
local holes = { 1, nil, 3 }
local nested = { { 1 }, { 2, { 3 } }, {} }
local calls = { sideEffect(), sideEffect() }
local _condition = n
if _condition == nil then
	_condition = 2
end
local withPrereq = { 1, _condition, 3 }
local _array = {}
local _length = #_array
table.move(arr, 1, #arr, _length + 1, _array)
local spreadOnly = _array
local _array_1 = { 1, 2 }
local _length_1 = #_array_1
table.move(arr, 1, #arr, _length_1 + 1, _array_1)
local spreadEnd = _array_1
local _array_2 = { 1 }
local _length_2 = #_array_2
local _arrLength = #arr
table.move(arr, 1, _arrLength, _length_2 + 1, _array_2)
_length_2 += _arrLength
_array_2[_length_2 + 1] = 2
_array_2[_length_2 + 2] = 3
local spreadMiddle = _array_2
local _array_3 = {}
local _length_3 = #_array_3
local _arrLength_1 = #arr
table.move(arr, 1, _arrLength_1, _length_3 + 1, _array_3)
_length_3 += _arrLength_1
local _array_4 = getArr()
table.move(_array_4, 1, #_array_4, _length_3 + 1, _array_3)
local spreadTwo = _array_3
local _array_5 = {}
local _length_4 = #_array_5
local _readonlyArrLength = #readonlyArr
table.move(readonlyArr, 1, _readonlyArrLength, _length_4 + 1, _array_5)
_length_4 += _readonlyArrLength
_array_5[_length_4 + 1] = 0
local spreadReadonly = _array_5
local _array_6 = {}
local _length_5 = #_array_6
for _char in string.gmatch(str, utf8.charpattern) do
	_length_5 += 1
	_array_6[_length_5] = _char
end
local spreadString = _array_6
local _array_7 = { "a" }
local _length_6 = #_array_7
for _char in string.gmatch(str, utf8.charpattern) do
	_length_6 += 1
	_array_7[_length_6] = _char
end
_array_7[_length_6 + 1] = "b"
local spreadStringMiddle = _array_7
local _array_8 = { 0 }
local _length_7 = #_array_8
for _v in set do
	_length_7 += 1
	_array_8[_length_7] = _v
end
_array_8[_length_7 + 1] = 1
local spreadSet = _array_8
local _array_9 = {}
local _length_8 = #_array_9
for _k, _v in map do
	_length_8 += 1
	_array_9[_length_8] = { _k, _v }
end
local spreadMap = _array_9
local _array_10 = {}
local _length_9 = #_array_10
for _result in gen.next do
	if _result.done then
		break
	end
	_length_9 += 1
	_array_10[_length_9] = _result.value
end
_array_10[_length_9 + 1] = 9
local spreadGenerator = _array_10
local _array_11 = {}
local _length_10 = #_array_11
for _result in iter do
	_length_10 += 1
	_array_11[_length_10] = _result
end
local spreadIterable = _array_11
local _array_12 = { 1 }
local _length_11 = #_array_12
local _iterFunc = tupleIter
while true do
	local _results = { _iterFunc() }
	if _results[1] == nil then
		break
	end
	_length_11 += 1
	_array_12[_length_11] = _results
end
_array_12[_length_11 + 1] = 2
local spreadTupleIterable = _array_12
local _array_13 = {}
local _length_12 = #_array_13
for _k, _v in shared do
	_length_12 += 1
	_array_13[_length_12] = { _k, _v }
end
local spreadShared = _array_13
local _array_14 = {}
local _length_13 = #_array_14
local _tupleLength = #tuple
table.move(tuple, 1, _tupleLength, _length_13 + 1, _array_14)
_length_13 += _tupleLength
_array_14[_length_13 + 1] = 1
local spreadTuple = _array_14
local _array_15 = {}
local _length_14 = #_array_15
local _array_16 = { 1, 2 }
local _Length = #_array_16
table.move(_array_16, 1, _Length, _length_14 + 1, _array_15)
_length_14 += _Length
_array_15[_length_14 + 1] = 3
local spreadLiteral = _array_15
local _array_17 = {}
local _length_15 = #_array_17
local _arrLength_2 = #arr
table.move(arr, 1, _arrLength_2, _length_15 + 1, _array_17)
_length_15 += _arrLength_2
local _condition_1 = n
if _condition_1 == nil then
	_condition_1 = 1
end
_array_17[_length_15 + 1] = _condition_1
_array_17[_length_15 + 2] = 2
local prereqAfterSpread = _array_17
local _array_18 = {}
local _length_16 = #_array_18
local _condition_2 = n
if _condition_2 == nil then
	_condition_2 = 1
end
_array_18[_length_16 + 1] = _condition_2
table.move(arr, 1, #arr, _length_16 + 2, _array_18)
local prereqBeforeSpread = _array_18
local _array_19 = {}
local _length_17 = #_array_19
for _char in string.gmatch("abc", utf8.charpattern) do
	_length_17 += 1
	_array_19[_length_17] = _char
end
local spreadStringLiteral = _array_19
local _object = table.clone({
	b = 2,
})
setmetatable(_object, nil)
local _array_20 = {}
local _length_18 = #_array_20
table.move(arr, 1, #arr, _length_18 + 1, _array_20)
local objectsInArray = { {
	a = 1,
}, _object, _array_20 }
return {
	simple = simple,
	empty = empty,
	holes = holes,
	nested = nested,
	calls = calls,
	withPrereq = withPrereq,
	spreadOnly = spreadOnly,
	spreadEnd = spreadEnd,
	spreadMiddle = spreadMiddle,
	spreadTwo = spreadTwo,
	spreadReadonly = spreadReadonly,
	spreadString = spreadString,
	spreadStringMiddle = spreadStringMiddle,
	spreadSet = spreadSet,
	spreadMap = spreadMap,
	spreadGenerator = spreadGenerator,
	spreadIterable = spreadIterable,
	spreadTupleIterable = spreadTupleIterable,
	spreadShared = spreadShared,
	spreadTuple = spreadTuple,
	spreadLiteral = spreadLiteral,
	prereqAfterSpread = prereqAfterSpread,
	prereqBeforeSpread = prereqBeforeSpread,
	spreadStringLiteral = spreadStringLiteral,
	objectsInArray = objectsInArray,
}
