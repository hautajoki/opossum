-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- classes without inheritance: fields, parameter properties, methods, statics
local NoConstructor, Empty, Shape, WithCallbacks
local function useClasses()
	print(NoConstructor, Empty, Shape, WithCallbacks)
end
local Point
do
	Point = setmetatable({}, {
		__tostring = function()
			return "Point"
		end,
	})
	Point.__index = Point
	function Point.new(...)
		local self = setmetatable({}, Point)
		return self:constructor(...) or self
	end
	function Point:constructor(y, z, w)
		if w == nil then
			w = 3
		end
		self.z = z
		self.w = w
		self.x = 0
		self.label = "point"
		-- a comment on the first statement of a constructor without super()
		self.y = y
		print("made", self.x, self.y)
	end
	function Point:length()
		return math.sqrt(self.x * self.x + self.y * self.y)
	end
	function Point:scaled(by)
		self.x *= by
		return self
	end
	function Point.describe()
		return "Point"
	end
	function Point:helper()
		return self.dimensions + Point.origin
	end
	Point["computed key"] = function(self)
		return 1
	end
	function Point:quoted()
		return 2
	end
	Point[10] = function(self)
		return 10
	end
	Point.later = TS.async(function(self)
		print("later")
	end)
	function Point:overloaded(a)
		return a
	end
	function Point:toString()
		return `Point({self.x}, {self.y})`
	end
	function Point:__tostring()
		return self:toString()
	end
	Point.origin = 0
	Point.dimensions = 2
	do
		print("static block", Point.dimensions)
		Point.origin = 1
	end
end
do
	NoConstructor = setmetatable({}, {
		__tostring = function()
			return "NoConstructor"
		end,
	})
	NoConstructor.__index = NoConstructor
	function NoConstructor.new(...)
		local self = setmetatable({}, NoConstructor)
		return self:constructor(...) or self
	end
	function NoConstructor:constructor()
		self.value = 5
		self.other = self.value + 1
	end
	function NoConstructor:method()
		return self.value
	end
end
do
	Empty = setmetatable({}, {
		__tostring = function()
			return "Empty"
		end,
	})
	Empty.__index = Empty
	function Empty.new(...)
		local self = setmetatable({}, Empty)
		return self:constructor(...) or self
	end
	function Empty:constructor()
	end
end
do
	Shape = {}
	function Shape:constructor()
	end
	function Shape:describe()
		return `area {self:area()}`
	end
end
do
	WithCallbacks = setmetatable({}, {
		__tostring = function()
			return "WithCallbacks"
		end,
	})
	WithCallbacks.__index = WithCallbacks
	function WithCallbacks.new(...)
		local self = setmetatable({}, WithCallbacks)
		return self:constructor(...) or self
	end
	function WithCallbacks:constructor()
		self.callback = function()
			return self.value
		end
		self.value = 1
	end
	WithCallbacks.staticCallback = function()
		return WithCallbacks.counter
	end
	WithCallbacks.counter = 0
	WithCallbacks.self = WithCallbacks
end
return {
	useClasses = useClasses,
	Point = Point,
}
