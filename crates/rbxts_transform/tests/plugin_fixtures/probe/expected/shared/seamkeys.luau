-- Compiled with roblox-ts v3.0.0
-- computed keys: offered to property_key with the key of a single string literal type, else `None` (declined)
local ka = "a"
local km = "m"
local c1 = keyed.k_a
keyName()
local c2 = keyed.k_b
keyed.k_a = 1
keyName()
keyed.k_b += 2
local c3 = keyed:k_m(1)
keyName()
local c4 = keyed:k_m(2)
local _self = keyed
local _c5 = _self.k_m
if _c5 ~= nil then
	_c5 = _c5(_self, 3)
end
local c5 = _c5
local _c6 = maybeKeyed
if _c6 ~= nil then
	_c6 = _c6.k_a
end
local c6 = _c6
local c7 = keyed[anyKey]
local c8 = keyedList[0]
keyName()
keyed.k_opt = nil
local c9 = "marked=marked:-;marked=marked:the marked one;maybeMarked=maybeMarked:piped text;keyed=keyed:-"
-- a rewritten key drops an argument that only reads (an identifier, an enum member); one with effects is still
-- evaluated, as a call statement when it is a call behind parentheses or a cast
local SK = {
	A = "a",
	M = "m",
}
local KeyNS = {}
do
	local _container = KeyNS
	local Inner = {
		B = "b",
	}
	_container.Inner = Inner
end
local c10 = keyed.k_a
local c11 = keyed:k_m(4)
local _c12 = maybeKeyed
if _c12 ~= nil then
	_c12 = _c12.k_b
end
local c12 = _c12
keyed.k_a = 5
keyName()
local c13 = keyed.k_a
local _ = keyed.k_s
local c14 = keyed.k_a
local c15 = keyed.k_b
-- optional chains with a rewritten computed key: the key is evaluated after its receiver, and not at all when an
-- optional receiver is nil
local kv = 0
local _self_1 = getMaybeKeyed()
local _o1 = _self_1
if _o1 ~= nil then
	keyName()
	_o1 = _o1.k_m
	if _o1 ~= nil then
		_o1 = _o1(_self_1, val())
	end
end
local o1 = _o1
local _o2 = getMaybeKeyed()
if _o2 ~= nil then
	keyName()
	_o2 = _o2.k_f
	if _o2 ~= nil then
		_o2 = _o2(val())
	end
end
local o2 = _o2
local _o3 = getMaybeKeyed()
if _o3 ~= nil then
	keyName()
	_o3 = _o3.k_a
end
local o3 = _o3
local _o4 = getMaybeKeyed()
if _o4 ~= nil then
	keyName()
	_o4 = _o4:k_m(val())
end
local o4 = _o4
local _self_2 = getKeyed()
keyName()
local _o5 = _self_2.k_m
if _o5 ~= nil then
	_o5 = _o5(_self_2, val())
end
local o5 = _o5
local _o6 = getMaybeKeyed()
if _o6 ~= nil then
	local _self_3 = _o6.k_inner
	keyName()
	_o6 = _self_3.k_m
	if _o6 ~= nil then
		_o6 = _o6(_self_3, val())
	end
end
local o6 = _o6
local _o7 = getMaybeKeyed()
if _o7 ~= nil then
	keyName()
	_o7 = _o7.k_h
	if _o7 ~= nil then
		local _self_3 = _o7(val())
		_o7 = _self_3
		if _o7 ~= nil then
			keyName()
			_o7 = _o7.k_g
			if _o7 ~= nil then
				_o7 = _o7(_self_3, val())
			end
		end
	end
end
local o7 = _o7
local _o8 = getMaybeKeyed()
if _o8 ~= nil then
	keyName()
	_o8 = _o8:k_g(val())
	if _o8 ~= nil then
		keyName()
		_o8 = _o8.k_h
		if _o8 ~= nil then
			local _exp = _o8(val())
			keyName()
			_o8 = _exp.k_a
		end
	end
end
local o8 = _o8
local _self_3 = getMaybeKeyed()
local _o9 = _self_3
if _o9 ~= nil then
	kv = 1
	local _1 = kv
	_o9 = _o9.k_m
	if _o9 ~= nil then
		_o9 = _o9(_self_3, val())
	end
end
local o9 = _o9
local _o10 = maybeKeyed
if _o10 ~= nil then
	keyName()
	_o10 = _o10.k_f
	if _o10 ~= nil then
		_o10 = _o10(val())
	end
end
local o10 = _o10
local _self_4 = getMaybeKeyed()
local _o11 = _self_4
if _o11 ~= nil then
	_o11 = _o11.k_m
	if _o11 ~= nil then
		_o11 = _o11(_self_4, val())
	end
end
local o11 = _o11
local _self_5 = getMaybeKeyed()
local _result = _self_5
if _result ~= nil then
	keyName()
	_result = _result.k_m
	if _result ~= nil then
		_result(_self_5, val())
	end
end
local _result_1 = getMaybeKeyed()
if _result_1 ~= nil then
	keyName()
	_result_1.k_opt = nil
end
-- assignments with a computed key: the hook compiles `key_node`
local CompKeys
do
	CompKeys = setmetatable({}, {
		__tostring = function()
			return "CompKeys"
		end,
	})
	CompKeys.__index = CompKeys
	function CompKeys.new(...)
		local self = setmetatable({}, CompKeys)
		return self:constructor(...) or self
	end
	function CompKeys:constructor()
		self.attrs = {
			count = 0,
			label = "",
		}
	end
	function CompKeys:run(k)
		local kc = "count"
		self:setAttribute("count", 2)
		self:setAttribute(k, 1)
	end
end
return {
	c1 = c1,
	c2 = c2,
	c3 = c3,
	c4 = c4,
	c5 = c5,
	c6 = c6,
	c7 = c7,
	c8 = c8,
	c9 = c9,
	c10 = c10,
	c11 = c11,
	c12 = c12,
	c13 = c13,
	c14 = c14,
	c15 = c15,
	o1 = o1,
	o2 = o2,
	o3 = o3,
	o4 = o4,
	o5 = o5,
	o6 = o6,
	o7 = o7,
	o8 = o8,
	o9 = o9,
	o10 = o10,
	o11 = o11,
	CompKeys = CompKeys,
}
