-- Compiled with roblox-ts v3.0.0
-- `super.m()` and `super.m?.()` claimed by symbol, and a `super` callee whose key `property_key` rewrites: each keeps
-- `self` as the receiver.
local Sub
do
	local super = SeamBase
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
	function Sub:constructor(value)
		super.constructor(self, value)
	end
	function Sub:m(v)
		return wrap(super.k_m(self, v))
	end
	function Sub:n(v)
		local _result = super.k_n
		if _result ~= nil then
			_result = _result(self, v, "injected", nil)
		end
		local _condition = _result
		if _condition == nil then
			_condition = -1
		end
		return _condition
	end
	function Sub:probeKeyed()
		return super.k_probeKeyed(self) + 1
	end
end
return {
	Sub = Sub,
}
