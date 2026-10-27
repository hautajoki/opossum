-- Compiled with roblox-ts v3.0.0
-- Parenthesized expressions in a `for` header's initializer and incrementor (roblox-ts lowers them without the
-- expression statement's `skipDownwards`). An unused optional chain or a value-less macro call there is no value:
-- stock roblox-ts crashes on it ("Cannot render None"); rbxts-rs drops it like the same statement without parentheses.
local function optionalChainInitializer()
	do
		local _result = maybe
		if _result ~= nil then
			_result:f()
		end
		while true do
			break
		end
	end
end
local function optionalChainIncrementor()
	do
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				local _result = maybe
				if _result ~= nil then
					_result:f()
				end
			else
				_shouldIncrement = true
			end
			if not (g() < 1) then
				break
			end
			g()
		end
	end
end
local function doubleParentheses()
	do
		local _result = maybe
		if _result ~= nil then
			_result:f()
		end
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				local _result_1 = maybe
				if _result_1 ~= nil then
					_result_1:f()
				end
			else
				_shouldIncrement = true
			end
			if not (g() < 2) then
				break
			end
			g()
		end
	end
end
local function macroWithoutValue()
	local arr = { 1 }
	do
		table.insert(arr, 2)
		while true do
			break
		end
	end
end
local function optionalMacroWithoutValue(o)
	do
		local _result = o.a
		if _result ~= nil then
			table.insert(_result, 1)
		end
		while true do
			break
		end
	end
end
-- with a value, stock keeps it, as `local _ = …`
local function valueInParentheses()
	do
		local _ = (g())
		while true do
			break
		end
	end
end
local function assignmentInParentheses()
	local x = 0
	do
		x = 1
		local _ = x
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				local _original = x
				x += 1
			else
				_shouldIncrement = true
			end
			break
		end
	end
end
return {
	optionalChainInitializer = optionalChainInitializer,
	optionalChainIncrementor = optionalChainIncrementor,
	doubleParentheses = doubleParentheses,
	macroWithoutValue = macroWithoutValue,
	optionalMacroWithoutValue = optionalMacroWithoutValue,
	valueInParentheses = valueInParentheses,
	assignmentInParentheses = assignmentInParentheses,
}
