-- Compiled with roblox-ts v3.0.0
-- for statements: the --optimizedLoops numeric for, and the while-loop fallback
local function numericLoops(n)
	local COUNT = 10
	local STEP = 2
	local NEGATIVE = -3
	for i = 0, 9 do
		print(i)
	end
	for i = 1, COUNT, 2 do
		print(i)
	end
	for i = 10, 1, -1 do
		print(i)
	end
	for i = 10, -5, -3 do
		print(i)
	end
	for i = 0, COUNT, STEP do
		print(i)
	end
	for i = 0, COUNT - 1, STEP do
		print(i)
	end
	for i = COUNT, 1, -STEP do
		print(i)
	end
	for i = 0, COUNT - 1, STEP do
		print(i)
	end
	for i = COUNT, 0, -STEP do
		print(i)
	end
	for i = 0, 100, 2 * 3 do
		print(i)
	end
	for i = 100, 0, -(2 * 3) do
		print(i)
	end
	for i = 0, -19, NEGATIVE do
		print(i)
	end
	for i = 0, (COUNT - 1) - 1 do
		print(i)
	end
	do
		local i = 0
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				i += 1
			else
				_shouldIncrement = true
			end
			if not (i < n) then
				break
			end
			print(i)
		end
	end
end
local function fallbackLoops(n)
	-- the body writes the variable
	do
		local _i = 0
		local _shouldIncrement = false
		while true do
			local i = _i
			if _shouldIncrement then
				i += 1
			else
				_shouldIncrement = true
			end
			if not (i < 10) then
				break
			end
			if i == 3 then
				i += 2
			end
			print(i)
			_i = i
		end
	end
	-- a wrong-way step
	do
		local i = 0
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				i -= 1
			else
				_shouldIncrement = true
			end
			if not (i < 10) then
				break
			end
			print(i)
			break
		end
	end
	-- a zero step
	do
		local i = 0
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				i += 0
			else
				_shouldIncrement = true
			end
			if not (i < 10) then
				break
			end
			print(i)
			break
		end
	end
	-- not an integer
	do
		local i = 0.5
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				i += 1
			else
				_shouldIncrement = true
			end
			if not (i < 10) then
				break
			end
			print(i)
		end
	end
	-- an unsupported comparison
	do
		local i = 0
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				i += 1
			else
				_shouldIncrement = true
			end
			if not (i ~= 10) then
				break
			end
			print(i)
		end
	end
	-- two variables
	do
		local i = 0
		local j = 10
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				local _original = i
				i += 1
				local _original_1 = j
				j -= 1
			else
				_shouldIncrement = true
			end
			if not (i < j) then
				break
			end
			print(i, j)
		end
	end
	-- no initializer, no incrementor
	local k = 0
	while k < n do
		k += 1
	end
	-- an expression initializer
	do
		k = 0
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				k += 1
			else
				_shouldIncrement = true
			end
			if not (k < n) then
				break
			end
			print(k)
		end
	end
	-- no condition
	do
		local i = 0
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				i += 1
			else
				_shouldIncrement = true
			end
			if i > n then
				break
			end
		end
	end
	-- nothing at all
	while true do
		if n > 0 then
			break
		end
	end
	-- a const variable
	do
		local value = n
		while value < 10 do
			print(value)
			break
		end
	end
end
local function capturedLoops(n)
	local callbacks = {}
	-- read from a function: a fresh copy per iteration
	do
		local _i = 0
		local _shouldIncrement = false
		while true do
			local i = _i
			if _shouldIncrement then
				i += 1
			else
				_shouldIncrement = true
			end
			if not (i < n) then
				break
			end
			table.insert(callbacks, function()
				return print(i)
			end)
			_i = i
		end
	end
	-- with continue, the copy is written back before every continue
	do
		local _i = 0
		local _shouldIncrement = false
		while true do
			local i = _i
			if _shouldIncrement then
				i += 1
			else
				_shouldIncrement = true
			end
			if not (i < n) then
				break
			end
			if i % 2 == 0 then
				_i = i
				continue
			elseif i % 3 == 0 then
				print("three")
				_i = i
				continue
			else
				table.insert(callbacks, function()
					return print(i)
				end)
			end
			repeat
				do
					if i > 5 then
						continue
					end
				end
			until not false
			_i = i
		end
	end
	-- used in its own initializer: a copy of the copy
	do
		local _iCopy = n
		local _j = _iCopy
		local _i = _iCopy
		local _shouldIncrement = false
		while true do
			local i = _i
			local j = _j
			if _shouldIncrement then
				i += 1
			else
				_shouldIncrement = true
			end
			if not (i < 10) then
				break
			end
			table.insert(callbacks, function()
				return print(i, j)
			end)
			_i = i
			_j = j
		end
	end
	-- written in the body
	do
		local _i = 0
		local _shouldIncrement = false
		while true do
			local i = _i
			if _shouldIncrement then
				i += 1
			else
				_shouldIncrement = true
			end
			if not (i < n) then
				break
			end
			i += 1
			if i > 4 then
				return nil
			end
			_i = i
		end
	end
	-- a step read from a function is not known to be initialized
	local STEP_LATER
	local run = function()
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += STEP_LATER
				else
					_shouldIncrement = true
				end
				if not (i < 10) then
					break
				end
				print(i)
			end
		end
	end
	STEP_LATER = 2
	run()
	return callbacks
end
local function loopConditions(values, text, flag)
	do
		local i = 0
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				i += 1
			else
				_shouldIncrement = true
			end
			if not (i < #values and values[i + 1] ~= 0) then
				break
			end
			print(values[i + 1])
		end
	end
	do
		local i = 0
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				i += 1
			else
				_shouldIncrement = true
			end
			if not (text ~= "" and text) then
				break
			end
			text = string.sub(text, 2)
		end
	end
	do
		local i = 0
		local _shouldIncrement = false
		while true do
			if _shouldIncrement then
				i += 1
			else
				_shouldIncrement = true
			end
			local _condition = flag
			if _condition == nil then
				_condition = i < 3
			end
			if not _condition then
				break
			end
			print(i)
		end
	end
	for i = 0, 9 do
		print(i)
	end
end
return {
	numericLoops = numericLoops,
	fallbackLoops = fallbackLoops,
	capturedLoops = capturedLoops,
	loopConditions = loopConditions,
}
