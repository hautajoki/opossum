-- Compiled with roblox-ts v3.0.0
-- the assignment hook: `this.attrs.<key> …` becomes `self:setAttribute("<key>", value[, true])`
local Comp
do
	Comp = setmetatable({}, {
		__tostring = function()
			return "Comp"
		end,
	})
	Comp.__index = Comp
	function Comp.new(...)
		local self = setmetatable({}, Comp)
		return self:constructor(...) or self
	end
	function Comp:constructor()
		self.attrs = {
			count = 0,
			label = "",
		}
		self.plain = {
			count = 0,
		}
	end
	function Comp:run()
		self:setAttribute("count", 1)
		self:setAttribute("count", self.attrs.count + 2)
		self:setAttribute("label", self.attrs.label .. "x")
		self:setAttribute("count", self.attrs.count + 1)
		local a = self:setAttribute("count", self.attrs.count + 1, true)
		local b = self:setAttribute("count", self.attrs.count + 1)
		local c = (self:setAttribute("count", 5))
		local _condition = self.attrs.flag
		if _condition == nil then
			_condition = true
		end
		self:setAttribute("flag", _condition)
		self:setAttribute("count", self.attrs.count - sideEffect())
		self:setAttribute("flag", nil)
		self.plain.count = 1
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					self:setAttribute("count", self.attrs.count + 1)
				else
					_shouldIncrement = true
				end
				if not (i < 2) then
					break
				end
				print(i)
			end
		end
		-- claimed inside parentheses of a `for` head: the parentheses wrap the value
		do
			local _ = (self:setAttribute("count", 0))
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					local _1 = (self:setAttribute("count", self.attrs.count + 1))
				else
					_shouldIncrement = true
				end
				if not (self.attrs.count < 1) then
					break
				end
				print(a)
			end
		end
		return { a, b, c }
	end
end
return {
	Comp = Comp,
}
