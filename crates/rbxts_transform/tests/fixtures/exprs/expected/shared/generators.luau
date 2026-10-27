-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- Hand-written T2 exprs fixture: yield and yield*.
local function counter(n)
	return TS.generator(function()
		coroutine.yield()
		coroutine.yield(n)
		local received = coroutine.yield(n + 1)
		print(received)
		return 5
	end)
end
local function delegating(n)
	return TS.generator(function()
		local _next = counter(n).next
		local _currentInput
		while true do
			local _result = _next(_currentInput)
			if _result.done then
				break
			end
			_currentInput = coroutine.yield(_result.value)
		end
		local _next_1 = counter(n + 1).next
		local _currentInput_1
		local _returnValue
		while true do
			local _result = _next_1(_currentInput_1)
			if _result.done then
				_returnValue = _result.value
				break
			end
			_currentInput_1 = coroutine.yield(_result.value)
		end
		local result = _returnValue
		print(result)
	end)
end
local arrowLike = function()
	return TS.generator(function()
		local x = coroutine.yield(1)
		coroutine.yield(x * 2)
	end)
end
return {
	counter = counter,
	delegating = delegating,
	arrowLike = arrowLike,
}
