-- Compiled with roblox-ts v3.0.0
local function objectParameter(_param)
	local a = _param.a
	local b = _param.b
	if b == nil then
		b = 2
	end
	return a + b
end
local function arrayParameters(_param, _param_1)
	local x = _param[1]
	local y = _param[2]
	if _param_1 == nil then
		_param_1 = {}
	end
	local z = _param_1[1]
	if z == nil then
		z = 3
	end
	return x + y + z
end
local function restTuple(p, q)
	return `{p}{q}`
end
local function restTupleNested(_, q, _param, _param_1)
	if q == nil then
		q = "q"
	end
	local r = _param[1]
	local s = _param_1.s
	return { q, r, s }
end
local function restWithSpread(...)
	local _param = { ... }
	local p = _param[1]
	local more = table.move(_param, 2, #_param, 1, {})
	return p + #more
end
local function nestedParameter(_param)
	local _binding = _param.inner
	local deep = _binding.deep
	local _binding_1 = _param.list
	local first = _binding_1[1]
	local others = table.move(_binding_1, 2, #_binding_1, 1, {})
	return deep .. first .. tostring(#others)
end
local function objectRestParameter(_param)
	local a = _param.a
	local _rest = {}
	for _key, _value in _param do
		if _key ~= "a" then
			_rest[_key] = _value
		end
	end
	local rest = _rest
	return { a, rest }
end
local function defaultedPattern(_param, _param_1)
	if _param == nil then
		_param = {
			a = 1,
		}
	end
	local a = _param.a
	if _param_1 == nil then
		_param_1 = { 2 }
	end
	local b = _param_1[1]
	return a + b
end
local arrow = function(_param, _param_1)
	local a = _param.a
	local b = _param_1[1]
	return a + b
end
local method = {
	run = function(self, _param)
		local value = _param.value
		return value
	end,
}
return {
	objectParameter = objectParameter,
	arrayParameters = arrayParameters,
	restTuple = restTuple,
	restTupleNested = restTupleNested,
	restWithSpread = restWithSpread,
	nestedParameter = nestedParameter,
	objectRestParameter = objectRestParameter,
	defaultedPattern = defaultedPattern,
	arrow = arrow,
	method = method,
}
