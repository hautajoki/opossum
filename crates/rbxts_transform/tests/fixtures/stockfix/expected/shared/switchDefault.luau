-- Compiled with roblox-ts v3.0.0
-- A `default` that is not the last clause. roblox-ts lowered it as the end of the switch: every case after it was
-- dropped, so it ran the default where JavaScript runs a later case.
local function middle(x)
	local log = {}
	repeat
		local _fallthrough = false
		if x == 1 then
			table.insert(log, "one")
			_fallthrough = true
		end
		local _case
		if not _fallthrough then
			if x == 2 then
				_case = 1
			elseif x == 3 then
				_case = 2
			end
		end
		if _case == nil then
			table.insert(log, "default")
			_fallthrough = true
		end
		if _fallthrough or _case == 1 then
			table.insert(log, "two")
			break
		end
		if _case == 2 then
			table.insert(log, "three")
		end
	until true
	return log
end
local function first(x)
	local log = {}
	repeat
		local _fallthrough = false
		local _case
		if x == 1 then
			_case = 1
		elseif x == 2 then
			_case = 2
		end
		if _case == nil then
			table.insert(log, "default")
			break
		end
		if _case == 1 then
			_fallthrough = true
		end
		if _fallthrough or _case == 2 then
			table.insert(log, "one or two")
		end
	until true
	return log
end
-- later cases are tested in order, once each, and only until one matches
local function testedOnce(x)
	local log = {}
	local test = function(value)
		table.insert(log, `test {value}`)
		return value
	end
	local _exp = x
	repeat
		local _caseValue = test(1)
		if _exp == _caseValue then
			table.insert(log, "one")
			break
		end
		local _case
		local _caseValue_1 = test(2)
		if _exp == _caseValue_1 then
			_case = 1
		else
			table.insert(log, "prereq")
			local _ = #log
			if _exp == 3 then
				_case = 2
			end
		end
		if _case == nil then
			table.insert(log, "default")
			break
		end
		if _case == 1 then
			table.insert(log, "two")
			break
		end
		if _case == 2 then
			table.insert(log, "three")
		end
	until true
	return log
end
return {
	middle = middle,
	first = first,
	testedOnce = testedOnce,
}
