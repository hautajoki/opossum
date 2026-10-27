-- Compiled with roblox-ts v3.0.0
local WithCall
do
	WithCall = setmetatable({}, {
		__tostring = function()
			return "WithCall"
		end,
	})
	WithCall.__index = WithCall
	function WithCall.new(...)
		local self = setmetatable({}, WithCall)
		return self:constructor(...) or self
	end
	function WithCall:constructor()
	end
	WithCall.__probed = "probe"
	WithCall = probeDecoSelf("x")(WithCall) or WithCall
end
local WithoutCall
do
	WithoutCall = setmetatable({}, {
		__tostring = function()
			return "WithoutCall"
		end,
	})
	WithoutCall.__index = WithoutCall
	function WithoutCall.new(...)
		local self = setmetatable({}, WithoutCall)
		return self:constructor(...) or self
	end
	function WithoutCall:constructor()
	end
	WithoutCall.__probed = "probe"
	WithoutCall = probeDecoSelfPlain(WithoutCall) or WithoutCall
end
return {
	WithCall = WithCall,
	WithoutCall = WithoutCall,
}
