-- Compiled with roblox-ts v3.0.0
-- Hand-written literals fixture: spread in call arguments.
take(unpack(arr))
take(1, 2, unpack(arr))
local _array = {}
local _length = 0
for _v in set do
	_length += 1
	_array[_length] = _v
end
take(unpack(_array))
local _array_1 = {}
local _length_1 = 0
for _char in string.gmatch(str, utf8.charpattern) do
	_length_1 += 1
	_array_1[_length_1] = _char
end
takeStrings(unpack(_array_1))
local _array_2 = {}
local _length_2 = 0
for _k, _v in map do
	_length_2 += 1
	_array_2[_length_2] = { _k, _v }
end
takeAny(unpack(_array_2))
take(unpack({ 1, 2 }))
print(unpack(arr))
local function forward(...)
	local args = { ... }
	take(unpack(args))
	local _array_3 = {}
	local _length_3 = #_array_3
	table.move(args, 1, #args, _length_3 + 1, _array_3)
	return _array_3
end
return {
	forward = forward,
}
