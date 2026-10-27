-- Compiled with roblox-ts v3.0.0
local Decorated
do
	Decorated = setmetatable({}, {
		__tostring = function()
			return "Decorated"
		end,
	})
	Decorated.__index = Decorated
	function Decorated.new(...)
		local self = setmetatable({}, Decorated)
		return self:constructor(...) or self
	end
	function Decorated:constructor()
	end
	Decorated.count = 1
	Decorated.__probed = "probe"
	Decorated = probeDecorator("probed")(Decorated) or Decorated
end
local Plain
do
	Plain = setmetatable({}, {
		__tostring = function()
			return "Plain"
		end,
	})
	Plain.__index = Plain
	function Plain.new(...)
		local self = setmetatable({}, Plain)
		return self:constructor(...) or self
	end
	function Plain:constructor()
	end
	Plain.count = 2
end
local widget = ProbeWidget.new("probed", 5)
return {
	Decorated = Decorated,
	Plain = Plain,
	widget = widget,
}
