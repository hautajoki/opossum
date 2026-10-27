-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
local impl = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "runtime").impl
-- probe: header
local counter = 0
local function bump()
	counter += 1
	return counter
end
local _hoisted = { 1, 2 }
local hoisted = _hoisted
local _hoisted_1 = {
	size = counter,
}
local function useHoist()
	return _hoisted_1
end
-- probe: prereq
local _probed = bump()
local prereq = _probed
local _exp = counter
local _condition = maybe
if _condition == nil then
	_condition = bump()
end
local ordered = { _exp, _condition }
probeInject(bump(), "injected", nil)
local swapped = impl(`n{counter}`, 1)
local captured = (function()
	local _condition_1 = maybe
	if _condition_1 == nil then
		_condition_1 = bump()
	end
	return _condition_1
end)()
-- probe: before the root statement
local _ = nil
local late = { "probe#1", "probe#2" }
local stamped = "src/shared/calls.ts:19:24"
local built = "fixture #1/1"
local unknown = mystery()
local plain = bump()
return {
	hoisted = hoisted,
	useHoist = useHoist,
	prereq = prereq,
	ordered = ordered,
	swapped = swapped,
	captured = captured,
	late = late,
	stamped = stamped,
	built = built,
	unknown = unknown,
	plain = plain,
}
