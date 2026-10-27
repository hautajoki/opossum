-- Compiled with roblox-ts v3.0.0
-- Temporary names that would capture or hide the program's own names (roblox-ts named each temporary as the
-- program's identifier).
-- a case block's `_fallthrough` hid the switch's flag from `_fallthrough = true`, so case 2 did not run
local function fallthroughCapture(x)
	local log = {}
	repeat
		local _fallthrough_1 = false
		if x == 1 then
			local _fallthrough = "user"
			table.insert(log, _fallthrough)
			_fallthrough_1 = true
		end
		if _fallthrough_1 or x == 2 then
			table.insert(log, "two")
			break
		end
	until true
	return log
end
-- a temporary met only in a `for … in` expression got the fallback name `_0`, which hid the outer `_0`
local function fallbackCapture()
	local _0 = 10
	local out = {}
	for _, v in (function()
		local _1 = maybe()
		if _1 ~= nil then
			_1 = _1.value
		end
		local r = _1
		local _2 = r
		if _2 == nil then
			_2 = _0
		end
		return { _2 }
	end)() do
		table.insert(out, v)
	end
	return out
end
-- a temporary in a loop body named as the loop variable, which the body reads after it
local function loopVariable(list)
	local out = {}
	for _, _n in list do
		local _n_1 = _n.inner
		if _n_1 ~= nil then
			_n_1 = _n_1.value
		end
		local n = _n_1
		local _condition = n
		if _condition == nil then
			_condition = 0
		end
		table.insert(out, _condition + _n.base)
	end
	return out
end
return {
	fallthroughCapture = fallthroughCapture,
	fallbackCapture = fallbackCapture,
	loopVariable = loopVariable,
}
