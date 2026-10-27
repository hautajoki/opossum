-- Compiled with roblox-ts v3.0.0
-- A computed key's prerequisites in an optional chain. roblox-ts ran them before the receiver's nil check, so
-- `x?.[(kv = 1, "m")]?.()` ran `kv = 1` even when `x` was nil.
local kv = 0
local function keyPrerequisites(x)
	kv = 0
	local _r = x
	if _r ~= nil then
		kv = 1
		local _ = kv
		_r = _r.m
		if _r ~= nil then
			_r = _r()
		end
	end
	local r = _r
	local _condition = r
	if _condition == nil then
		_condition = -1
	end
	return { _condition, kv }
end
-- a receiver that is not optional: the key comes after it
local function keyAfterReceiver()
	local holder = {
		m = function()
			return "first"
		end,
	}
	local second = {
		m = function()
			return "second"
		end,
	}
	local _exp = holder
	holder = second
	local _ = holder
	local _r = _exp.m
	if _r ~= nil then
		_r = _r()
	end
	local r = _r
	return r
end
return {
	keyPrerequisites = keyPrerequisites,
	keyAfterReceiver = keyAfterReceiver,
}
