-- Compiled with roblox-ts v3.0.0
-- `join` on an optional chain. roblox-ts read the receiver's type with `undefined` in it, found no element type, and
-- skipped the `tostring` loop, so `table.concat` failed on non-string values.
local function optionalBooleans(flags)
	local _result = flags
	if _result ~= nil then
		-- ▼ ReadonlyArray.join ▼
		local _result_1 = table.create(#_result)
		for _k, _v in _result do
			_result_1[_k] = tostring(_v)
		end
		-- ▲ ReadonlyArray.join ▲
		_result = table.concat(_result_1, ",")
	end
	return _result
end
-- strings need no loop, with or without the optional chain
local function optionalStrings(names)
	local _result = names
	if _result ~= nil then
		_result = table.concat(_result, "+")
	end
	return _result
end
return {
	optionalBooleans = optionalBooleans,
	optionalStrings = optionalStrings,
}
