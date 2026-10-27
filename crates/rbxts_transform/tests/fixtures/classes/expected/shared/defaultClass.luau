-- Compiled with roblox-ts v3.0.0
-- an anonymous default-exported class
local default
do
	default = setmetatable({}, {
		__tostring = function()
			return "default"
		end,
	})
	default.__index = default
	function default.new(...)
		local self = setmetatable({}, default)
		return self:constructor(...) or self
	end
	function default:constructor()
	end
	function default:method()
		return 1
	end
	default.value = 1
end
return {
	default = default,
}
