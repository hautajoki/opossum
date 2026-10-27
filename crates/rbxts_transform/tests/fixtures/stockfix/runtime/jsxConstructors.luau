return {
	ambient = {
		Vector2 = {
			new = function(x, y)
				return { X = x, Y = y }
			end,
			zero = { X = 0, Y = 0 },
		},
		CFrame = { identity = "CFrame.identity" },
		ColorSequence = {
			new = function(keypoints)
				return { count = #keypoints }
			end,
		},
		NumberRange = {
			new = function(min, max)
				assert(max >= min, "NumberRange: invalid range")
				return { min, max }
			end,
		},
		ColorSequenceKeypoint = {
			new = function(time, color)
				return { time = time, color = color }
			end,
		},
		Color3 = {
			fromRGB = function(r, g, b)
				return { r, g, b }
			end,
			fromHex = function(hex)
				assert(string.match(hex, "^#?%x+$"), "Unable to convert characters to hex value")
				return { hex }
			end,
		},
		Enum = { AutomaticSize = { Y = "AutomaticSize.Y" } },
		UDim2 = {
			new = function(xs, xo, ys, yo)
				return { xs, xo, ys, yo }
			end,
			fromScale = function(x, y)
				return { x, 0, y, 0 }
			end,
		},
		Instance = {
			new = function(className)
				return { ClassName = className }
			end,
		},
	},
	calls = {
		{
			call = "pureChild",
			expect = function(element)
				local child = element.children[1]
				return element.props.Size == 1 and child.X == 1 and child.Y == 2
			end,
		},
		{ call = "userCallChild", expect = { { Size = 1 }, 2 } },
		{
			call = "instanceChild",
			expect = function(element)
				return element.props.Size == 1 and element.children[1].ClassName == "Part"
			end,
		},
		{
			call = "pureAttributes",
			expect = function(element)
				local p = element.props
				return p.Size[2] == 1 and p.Size[4] == 2 and p.Position[1] == 0.5 and element.children[1].X == 0
			end,
		},
		{ call = "shadowed", expect = { { Size = 1 }, 3 } },
		{
			call = "constants",
			expect = function(element)
				local p = element.props
				return p.AutomaticSize == "AutomaticSize.Y" and p.AnchorPoint.X == 0 and p.CFrame == "CFrame.identity"
			end,
		},
		{ call = "shadowedEnum", expect = { { AutomaticSize = 1 }, 2 } },
		{
			call = "augmentedConstructor",
			expect = function(element)
				return element.props.Seq.count == 2
			end,
		},
		{
			call = "inheritedConstructor",
			expect = function(result)
				return result[1].Seq.count == result[2] - 1
			end,
		},
		{ call = "throwingRange", expect = { "threw" } },
		{ call = "throwingHex", expect = { "threw" } },
		{
			call = "aliasedEnum",
			expect = function(element)
				return element.props.AutomaticSize == "AutomaticSize.Y"
			end,
		},
	},
}
