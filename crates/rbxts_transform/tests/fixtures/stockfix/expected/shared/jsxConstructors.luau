-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- The capture of `jsxQuirks`' `attributesBeforeChildren` is left out when the later prerequisites can't change what
-- was read first: a Roblox datatype constructor (`new Vector2(…)`, `UDim2.fromScale(…)`) only allocates, and an Enum
-- item or a datatype constant (`Enum.AutomaticSize.Y`, `Vector2.zero`) is a constant. A user's call or `Instance.new`
-- in the same place still captures. (The fixture's tsconfig sets `preserveSymlinks`, so the linked `@rbxts/types`
-- counts as `@rbxts/types` and its globals are recognised.)
local React = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "react")
local box = {
	size = 1,
}
-- `unset ?? x` checks `unset` against nil in a prerequisite (`false` is a value), so the children have prerequisites
local unset = nil
local function resize()
	box.size = 2
	return Vector2.new(0, 0)
end
-- the children's `Vector2.new` can't change `box.size`: the table is built in place
local function pureChild()
	box.size = 1
	local _condition = unset
	if _condition == nil then
		_condition = Vector2.new(1, 2)
	end
	return React.createElement("frame", {
		Size = box.size,
	}, _condition)
end
-- a user's call can: the table is captured before it, with the size from before the call
local function userCallChild()
	box.size = 1
	local _attributes = {
		Size = box.size,
	}
	local _condition = unset
	if _condition == nil then
		_condition = resize()
	end
	local element = React.createElement("frame", _attributes, _condition)
	return { element.props, box.size }
end
-- `Instance.new` is not on the list (a new instance can run code): captured as well
local function instanceChild()
	box.size = 1
	local _attributes = {
		Size = box.size,
	}
	local _condition = unset
	if _condition == nil then
		_condition = Instance.new("Part")
	end
	return React.createElement("frame", _attributes, _condition)
end
-- constructors in the attributes: nothing the children's call does can change them, so no capture
local function pureAttributes()
	local _condition = unset
	if _condition == nil then
		_condition = resize()
	end
	return React.createElement("frame", {
		Size = UDim2.new(0, 1, 0, 2),
		Position = UDim2.fromScale(0.5, 0.5),
	}, _condition)
end
-- a local named like a datatype is not the datatype: `UDim2.fromScale` here is the user's function
local function shadowed()
	local UDim2 = {
		fromScale = function(x, y)
			box.size = 3
			return x + y
		end,
	}
	box.size = 1
	local _attributes = {
		Size = box.size,
	}
	local _condition = unset
	if _condition == nil then
		_condition = UDim2.fromScale(1, 2)
	end
	local element = React.createElement("frame", _attributes, _condition)
	return { element.props, box.size }
end
-- Enum items and datatype constants are constants: no capture before the children's call
local function constants()
	local _condition = unset
	if _condition == nil then
		_condition = resize()
	end
	return React.createElement("frame", {
		AutomaticSize = Enum.AutomaticSize.Y,
		AnchorPoint = Vector2.zero,
		CFrame = CFrame.identity,
	}, _condition)
end
-- a local `Enum` is the user's table, which the children's call changes: captured
local function shadowedEnum()
	local Enum = {
		AutomaticSize = {
			Y = 1,
		},
	}
	local grow = function()
		Enum.AutomaticSize.Y = 2
		return 0
	end
	local _attributes = {
		AutomaticSize = Enum.AutomaticSize.Y,
	}
	local _condition = unset
	if _condition == nil then
		_condition = grow()
	end
	local element = React.createElement("frame", _attributes, _condition)
	return { element.props, Enum.AutomaticSize.Y }
end
-- an Enum item reached through a variable is only that variable's member: captured
local function aliasedEnum()
	local items = Enum.AutomaticSize
	local _attributes = {
		AutomaticSize = items.Y,
	}
	local _condition = unset
	if _condition == nil then
		_condition = resize()
	end
	return React.createElement("frame", _attributes, _condition)
end
-- a user's augmentation can give a datatype a constructor that reads a table: only signatures `@rbxts/types`
-- declares count, so this one captures
local keypoints = { ColorSequenceKeypoint.new(0, Color3.fromRGB(0, 0, 0)), ColorSequenceKeypoint.new(1, Color3.fromRGB(255, 255, 255)) }
local holder = {
	bump = function()
		table.insert(keypoints, keypoints[1])
		return 0
	end,
}
local maybeHolder = holder
local function augmentedConstructor()
	local list = keypoints
	local _attributes = {
		Seq = ColorSequence.new(list),
	}
	local _result = maybeHolder
	if _result ~= nil then
		_result = _result.bump()
	end
	return React.createElement("frame", _attributes, _result)
end
-- a constructor the type inherits from another `@rbxts/types` interface is not the datatype's own: captured
local function inheritedConstructor()
	local list = keypoints
	local _attributes = {
		Seq = ColorSequence.new(list),
	}
	local _result = maybeHolder
	if _result ~= nil then
		_result = _result.bump()
	end
	local element = React.createElement("frame", _attributes, _result)
	return { element.props, #keypoints }
end
local log = {}
local sideEffect = {
	run = function()
		table.insert(log, "side effect")
		return 0
	end,
}
local maybeSide = sideEffect
-- a constructor that rejects a value of the right type throws before the children run, as in JavaScript
local function throwingRange()
	table.clear(log)
	TS.try(function()
		local _attributes = {
			Range = NumberRange.new(5, 1),
		}
		local _result = maybeSide
		if _result ~= nil then
			_result = _result.run()
		end
		React.createElement("frame", _attributes, _result)
		table.insert(log, "built")
	end, function(e)
		table.insert(log, "threw")
	end)
	local _array = {}
	local _length = #_array
	table.move(log, 1, #log, _length + 1, _array)
	return _array
end
local function throwingHex()
	table.clear(log)
	TS.try(function()
		local _attributes = {
			Color = Color3.fromHex("zz"),
		}
		local _result = maybeSide
		if _result ~= nil then
			_result = _result.run()
		end
		React.createElement("frame", _attributes, _result)
		table.insert(log, "built")
	end, function(e)
		table.insert(log, "threw")
	end)
	local _array = {}
	local _length = #_array
	table.move(log, 1, #log, _length + 1, _array)
	return _array
end
return {
	pureChild = pureChild,
	userCallChild = userCallChild,
	instanceChild = instanceChild,
	pureAttributes = pureAttributes,
	shadowed = shadowed,
	constants = constants,
	shadowedEnum = shadowedEnum,
	aliasedEnum = aliasedEnum,
	augmentedConstructor = augmentedConstructor,
	inheritedConstructor = inheritedConstructor,
	throwingRange = throwingRange,
	throwingHex = throwingHex,
}
