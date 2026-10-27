-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- Hand-written T2 exprs fixture: mixed edge cases.
-- only one branch has prerequisites
local function oneSided(flag, bag)
	local i = 0
	local _result
	if flag then
		_result = 1
	else
		local _original = i
		i += 1
		_result = _original
	end
	local a = _result
	local _result_1
	if flag then
		local _condition = bag.inner.x
		if _condition == nil then
			_condition = 2
		end
		_result_1 = _condition
	else
		_result_1 = 3
	end
	local b = _result_1
	return a + b + i
end
-- statement conditional whose condition needs prerequisites and truthiness checks
local function statementCondition(bag)
	local i = 0
	local _condition = bag.x
	if _condition == nil then
		local _original = i
		i += 1
		_condition = _original
	end
	if _condition ~= 0 and _condition == _condition and _condition then
		print("a")
	else
		print("b")
	end
	local _value = n()
	if _value ~= 0 and _value == _value and _value then
		print("c")
	else
		local _ = nil
	end
end
-- delete as a value
local function deleteValue(bag)
	bag.inner.x = nil
	bag.x = nil
	print(true, true)
end
-- a tag with prerequisites, non-ASCII and long-bracket text
local function tagEdges(bag)
	local i = 0
	local _original = i
	i += 1
	local r = (if _original > 0 then pick() else tag)({ "héllo ", " 😀 ]] end" }, i)
	local s = tag({ "a ", "", "" }, "x", bag.x)
	local t = tag({ 'back\\slash "quoted" ', "	'single'" }, s)
	return r .. s .. t
end
-- logical assignments inside arguments
local function argOrder(bag, s)
	local i = 0
	local _original = i
	i += 1
	if bag.x == nil then
		bag.x = i
	end
	local _exp = (bag.x)
	local _condition = s
	if not (s ~= "" and s) then
		_condition = "q"
	end
	s = _condition
	print(_original, _exp, s, i)
end
-- yield inside other expressions
local inner
local function yields(flag)
	return TS.generator(function()
		local v = if flag then coroutine.yield(1) else 2
		print((coroutine.yield(v)))
		local _next = inner().next
		local _currentInput
		local _returnValue
		while true do
			local _result = _next(_currentInput)
			if _result.done then
				_returnValue = _result.value
				break
			end
			_currentInput = coroutine.yield(_result.value)
		end
		local w = _returnValue
		return w
	end)
end
function inner()
	return TS.generator(function()
		coroutine.yield(1)
		return 2
	end)
end
return {
	oneSided = oneSided,
	statementCondition = statementCondition,
	deleteValue = deleteValue,
	tagEdges = tagEdges,
	argOrder = argOrder,
	yields = yields,
}
