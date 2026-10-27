-- Compiled with roblox-ts v3.0.0
-- Optional method calls (`o?.m()`, `o.m?.()`, `o?.m?.()`) and non-method optional calls (`o?.f?.()`), claimed: the
-- receiver is evaluated once, and nothing of the call (its arguments, the claimed value) runs when the chain stops.
local function receiverOptional()
	local _result = getMarked()
	if _result ~= nil then
		_result = _result:probed(next())
	end
	return _result
end
local function methodOptional()
	local _self = getMaybeMethod()
	local _result = _self.probeMethod
	if _result ~= nil then
		_result = _self:probed(next())
	end
	return _result
end
local function bothOptional()
	local _self = getMaybeMaybeMethod()
	local _result = _self
	if _result ~= nil then
		_result = _result.probeMethod
		if _result ~= nil then
			_result = _self:probed(next())
		end
	end
	return _result
end
local function fnOptional()
	local _result = getHolder()
	if _result ~= nil then
		local _self = _result
		_result = _self.fn
		if _result ~= nil then
			_result = useRecv(_self, next())
		end
	end
	return _result
end
local function selfOptional()
	local _result = getHolder()
	if _result ~= nil then
		_result = _result.sf
		if _result ~= nil then
			_result = wrap(_result(next()))
		end
	end
	return _result
end
local function statementOptional()
	local _result = getMarked()
	if _result ~= nil then
		_result:probed(next())
	end
end
return {
	receiverOptional = receiverOptional,
	methodOptional = methodOptional,
	bothOptional = bothOptional,
	fnOptional = fnOptional,
	selfOptional = selfOptional,
	statementOptional = statementOptional,
}
