-- Compiled with roblox-ts v3.0.0
-- Declined by the expression hook (probe-exprcall), then claimed by the call hook, which compiles the node itself:
-- the expression hook is asked once per node.
local function plain()
	return wrap(probeAskOnce(next()))
end
local function inChain()
	local _result = getAskOnce()
	if _result ~= nil then
		_result = wrap(_result:probeAskOnceM(next()))
	end
	return _result
end
local function construct()
	return wrap(probeAskOnceNew.new(next()))
end
return {
	plain = plain,
	inChain = inChain,
	construct = construct,
}
