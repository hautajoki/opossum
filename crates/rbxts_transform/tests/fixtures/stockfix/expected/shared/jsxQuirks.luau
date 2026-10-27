-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- JSX that roblox-ts lowered differently from TypeScript's JSX semantics.
local React = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "react")
local _Inner = function(props)
	return React.createElement("frame")
end
-- a tag whose name starts with `_` is the component of that name (TypeScript's `isIntrinsicJsxName`), not a
-- host element named by a string
local function componentTag()
	return React.createElement(_Inner)
end
-- a dashed name is a host element, whatever its first letter (roblox-ts failed an assertion on `<Custom-frame />`,
-- which would stop the whole build of stock roblox-ts here)
local function dashedTag()
	return React.createElement("custom-frame")
end
-- attribute strings have no escapes, decode HTML entities, and may span lines
local function attributeStrings()
	return React.createElement("textlabel", {
		Text = "a\\nb & c",
		Alt = "it's",
		Path = "C:\\temp",
		Lines = [[one
two]],
		Plain = 'single',
	})
end
local size = 1
local function bump()
	size = 2
	return nil
end
local holder = {
	bump = function()
		return bump()
	end,
}
-- the attributes are evaluated before the children
local function attributesBeforeChildren()
	size = 1
	local _attributes = {
		Size = size,
	}
	local _result = holder
	if _result ~= nil then
		_result = _result:bump()
	end
	return React.createElement("frame", _attributes, _result)
end
local First = function(props)
	return React.createElement("frame")
end
local Second = function(props)
	return React.createElement("frame")
end
local Tag = First
-- the tag is evaluated before the attributes
local function tagBeforeAttributes()
	Tag = First
	local _tag = Tag
	local _attributes = {}
	local _result = holder
	if _result ~= nil then
		_result = _result:bump()
	end
	local _condition = _result
	if _condition == nil then
		Tag = Second
		local _ = Tag
		_condition = 1
	end
	_attributes.Value = _condition
	local element = React.createElement(_tag, _attributes)
	return { element.tag == First, element.tag == Second }
end
return {
	componentTag = componentTag,
	dashedTag = dashedTag,
	attributeStrings = attributeStrings,
	attributesBeforeChildren = attributesBeforeChildren,
	tagBeforeAttributes = tagBeforeAttributes,
}
