-- Compiled with roblox-ts v3.0.0
-- optional method calls by receiver: the receiver is `_self`, the call lowered inside the nil check
local _self = marked
local _o1 = _self.probeMethod
if _o1 ~= nil then
	_o1 = _self:probed(1)
end
local o1 = _o1
local _self_1 = maybeMarked
local _o2 = _self_1
if _o2 ~= nil then
	_o2 = _o2.probeMethod
	if _o2 ~= nil then
		_o2 = _self_1:probed(2)
	end
end
local o2 = _o2
local _self_2 = marked
local _result = _self_2.probeMethod
if _result ~= nil then
	_self_2:probed(3)
end
-- non-method optional calls by symbol: compile_receiver binds the receiver before the callee is read from it
local _self_3 = holder
local _f1 = _self_3.fn
if _f1 ~= nil then
	_f1 = useRecv(_self_3, 1)
end
local f1 = _f1
local _self_4 = getHolder().inner
local _f2 = _self_4.fn
if _f2 ~= nil then
	_f2 = useRecv(_self_4, sideEffect())
end
local f2 = _f2
local _f3 = maybeHolder
if _f3 ~= nil then
	local _self_5 = _f3
	_f3 = _self_5.fn
	if _f3 ~= nil then
		_f3 = useRecv(_self_5, 3)
	end
end
local f3 = _f3
local _self_5 = holder
local _f4 = _self_5.mm
if _f4 ~= nil then
	_f4 = useRecv(_self_5, 4)
end
local f4 = _f4
local _f5 = holder.sf
if _f5 ~= nil then
	_f5 = wrap(_f5(5))
end
local f5 = _f5
local _self_6 = holder
local _f6 = _self_6.fn
if _f6 ~= nil then
	_f6 = useRecv(_self_6, 6)
end
local f6 = _f6
-- super calls by symbol
local Sub
do
	local super = SelfBase
	Sub = setmetatable({}, {
		__tostring = function()
			return "Sub"
		end,
		__index = super,
	})
	Sub.__index = Sub
	function Sub.new(...)
		local self = setmetatable({}, Sub)
		return self:constructor(...) or self
	end
	function Sub:constructor(...)
		super.constructor(self, ...)
	end
	function Sub:m(v)
		return wrap(super.m(self, v))
	end
	function Sub:n(v)
		local _result_1 = super.n
		if _result_1 ~= nil then
			_result_1 = _result_1(self, v, "injected", nil)
		end
		local _condition = _result_1
		if _condition == nil then
			_condition = 0
		end
		return _condition
	end
end
return {
	o1 = o1,
	o2 = o2,
	f1 = f1,
	f2 = f2,
	f3 = f3,
	f4 = f4,
	f5 = f5,
	f6 = f6,
	Sub = Sub,
}
