-- Compiled with roblox-ts v3.0.0
-- Accesses and calls inside chains, offered to the expression hooks (probe-exprcall): before anything of the chain is
-- compiled (before the first `?.`), or inside the nil check on their compiled base.
local function lazyLink()
	local _result = exprWrap(getA2().probeAccB)
	if _result ~= nil then
		_result = _result.c
	end
	return _result
end
local function eagerLink()
	local _result = getA()
	if _result ~= nil then
		_result = exprWrap(_result.probeAccB).c
	end
	return _result
end
local function takenLink()
	local _result = getA()
	if _result ~= nil then
		_result = takenValue.c
	end
	return _result
end
local function elementLink()
	local _result = getA()
	if _result ~= nil then
		_result = exprWrap(_result.probeAccB).c
	end
	return _result
end
local function deepLink()
	local _result = getA()
	if _result ~= nil then
		_result = _result.plain
		if _result ~= nil then
			_result = exprWrap(_result.probeAccB).c
		end
	end
	return _result
end
local function callInChain()
	local _result = getExprs()
	if _result ~= nil then
		_result = exprWrap(_result:probeExprM(next())).value
	end
	return _result
end
return {
	lazyLink = lazyLink,
	eagerLink = eagerLink,
	takenLink = takenLink,
	elementLink = elementLink,
	deepLink = deepLink,
	callInChain = callInChain,
}
