-- Compiled with roblox-ts v3.0.0
-- Hand-written T2 exprs fixture: `||=`, `&&=`, `??=`.
local function identifiers(x, y, z)
	if x == nil then
		x = 5
	end
	local _condition = y
	if not (y ~= "" and y) then
		_condition = "default"
	end
	y = _condition
	local _condition_1 = z
	if z then
		_condition_1 = false
	end
	z = _condition_1
	print(x, y, z)
end
local function properties(h)
	if h.n == nil then
		h.n = 1
	end
	local _condition = h.s
	local _value = h.s
	if not (_value ~= "" and _value) then
		_condition = "s"
	end
	h.s = _condition
	local _condition_1 = h.b
	if h.b then
		_condition_1 = h.s == "x"
	end
	h.b = _condition_1
	local _exp = getHolder().child
	if _exp.n == nil then
		_exp.n = make()
	end
end
local function elements(r)
	local _index = key()
	if r[_index] == nil then
		r[_index] = 2
	end
	local _condition = r.a
	local _value = r.a
	if not (_value ~= 0 and _value == _value and _value) then
		_condition = 3
	end
	r.a = _condition
end
-- used as values
local function asValues(h, x)
	if x == nil then
		x = 7
	end
	local a = x
	local _condition = h.s
	local _value = h.s
	if not (_value ~= "" and _value) then
		_condition = "q"
	end
	h.s = _condition
	local b = (h.s)
	local _condition_1 = h.b
	if h.b then
		_condition_1 = true
	end
	h.b = _condition_1
	print(a, b, (h.b))
	local _exp = h.child
	if _exp.n == nil then
		_exp.n = make()
	end
	return _exp.n
end
-- a right side with prerequisites
local function rightPrereqs(h)
	local i = 0
	if h.n == nil then
		local _original = i
		i += 1
		h.n = _original
	end
	local _condition = h.s
	local _value = h.s
	if not (_value ~= "" and _value) then
		local _original = i
		i += 1
		_condition = `{_original}`
	end
	h.s = _condition
	return i
end
return {
	identifiers = identifiers,
	properties = properties,
	elements = elements,
	asValues = asValues,
	rightPrereqs = rightPrereqs,
}
