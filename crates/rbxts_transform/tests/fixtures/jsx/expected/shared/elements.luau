-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- Hand-written JSX fixture: elements, attributes, spread props, fragments and children.
-- `React` is used only by JSX here, so its import must stay (the checker marks the JSX factory as referenced).
local React = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "react")
local Components = {
	Panel = function(panelProps)
		return React.createElement("frame", {
			Name = panelProps.Title,
		})
	end,
}
local _Inner = function(innerProps)
	return React.createElement("frame")
end
local counter = 0
-- function declarations first: TypeScript 5 lists them first among the exports, TypeScript 7 in source order
local function Button(buttonProps)
	return React.createElement("textbutton", {
		Text = buttonProps.Text,
		LayoutOrder = buttonProps.Order,
	}, buttonProps.children)
end
local function List()
	-- children arrays from map, keyed
	local _arg0 = function(item, i)
		return React.createElement("textlabel", {
			Key = item,
			Text = item,
			LayoutOrder = i,
		})
	end
	-- ▼ ReadonlyArray.map ▼
	local _newValue = table.create(#items)
	for _k, _v in items do
		_newValue[_k] = _arg0(_v, _k - 1, items)
	end
	-- ▲ ReadonlyArray.map ▲
	return React.createElement("frame", nil, _newValue)
end
local function Conditional()
	-- conditional children
	return React.createElement("frame", nil, enabled and React.createElement("textlabel", {
		Text = "on",
	}), if enabled then React.createElement("textlabel", {
		Text = "yes",
	}) else React.createElement("textlabel", {
		Text = "no",
	}), count > 0 and label ~= "" and React.createElement("textlabel", {
		Text = label,
	}), label ~= "" and React.createElement("textlabel", {
		Text = label,
	}), if maybeProps ~= nil then React.createElement("frame", {
		Size = maybeProps.Size,
	}) else nil)
end
-- host and component tags
local empty = React.createElement("frame")
local host = React.createElement("textlabel", {
	Text = "hello",
})
local component = React.createElement(Button, {
	Text = "press",
})
local componentWithChildren = (React.createElement(Button, {
	Text = "outer",
}, React.createElement("textlabel", {
	Text = "inner",
})))
local dotted = React.createElement(Components.Panel, {
	Title = "panel",
})
local nested = (React.createElement("screengui", nil, React.createElement("frame", {
	Size = 1,
}, React.createElement("textlabel", {
	Text = "deep",
}))))
-- attributes
local booleanAttribute = React.createElement("textbutton", {
	AutoButtonColor = true,
	Active = false,
})
local stringAttributes = React.createElement("textlabel", {
	Text = 'say "hi"',
	Name = "it's",
	Font = "a\\b",
})
local expressionAttributes = React.createElement("frame", {
	Size = count * 2,
	Name = `n{count}`,
	Visible = enabled,
})
local namespacedAttribute = React.createElement("frame", {
	["custom:value"] = 1,
})
local namespacedTag = React.createElement("ns:frame")
-- roblox-ts reads a tag starting with `_` as a host component, though TypeScript checks it as a value
local underscoreTag = React.createElement(_Inner)
local elementAttribute = React.createElement("frame", {
	Child = React.createElement("textlabel"),
})
-- attributes with prerequisites end the inline table
local _attributes = {}
local _original = counter
counter += 1
_attributes.LayoutOrder = _original
_attributes.Size = 1
local _original_1 = counter
counter += 1
_attributes.ZIndex = _original_1
local attributePrereqs = React.createElement("frame", _attributes)
local attributeCall = React.createElement("frame", {
	Name = getName(),
	Size = 1,
})
-- spread props
local _attributes_1 = table.clone(props)
setmetatable(_attributes_1, nil)
_attributes_1.LayoutOrder = 1
local spreadFirst = React.createElement("frame", _attributes_1)
local _attributes_2 = table.clone(props)
setmetatable(_attributes_2, nil)
local spreadOnly = React.createElement("frame", _attributes_2)
local _attributes_3 = {
	LayoutOrder = 1,
}
for _k, _v in props do
	_attributes_3[_k] = _v
end
local spreadAfter = React.createElement("frame", _attributes_3)
local _attributes_4 = table.clone(getProps())
setmetatable(_attributes_4, nil)
local spreadCall = React.createElement("frame", _attributes_4)
local _attributes_5 = {}
if maybeProps then
	for _k, _v in maybeProps do
		_attributes_5[_k] = _v
	end
end
_attributes_5.LayoutOrder = 2
local spreadMaybe = React.createElement("frame", _attributes_5)
local _attributes_6 = {
	LayoutOrder = 2,
}
local _attribute = getMaybeProps()
if _attribute then
	for _k, _v in _attribute do
		_attributes_6[_k] = _v
	end
end
local spreadMaybeCall = React.createElement("frame", _attributes_6)
local _attributes_7 = {
	ZIndex = 2,
}
local _attribute_1 = (enabled and {
	LayoutOrder = 3,
})
if _attribute_1 then
	for _k, _v in _attribute_1 do
		_attributes_7[_k] = _v
	end
end
local spreadConditional = React.createElement("frame", _attributes_7)
local _attributes_8 = table.clone(props)
setmetatable(_attributes_8, nil)
if maybeProps then
	for _k, _v in maybeProps do
		_attributes_8[_k] = _v
	end
end
_attributes_8.Key = "twice"
local spreadTwice = React.createElement("frame", _attributes_8)
local _attributes_9 = table.clone({
	Size = 1,
})
setmetatable(_attributes_9, nil)
local spreadObject = React.createElement("frame", _attributes_9)
-- children
local keyedChildren = (React.createElement("frame", nil, React.createElement("textlabel", {
	Key = "First",
	Text = "1",
}), React.createElement("textlabel", {
	Key = "Second",
	Text = "2",
})))
local arrayChild = React.createElement("frame", nil, elements)
local spreadChild = React.createElement("frame", nil, unpack(elements))
local _exp = React.createElement("textlabel", {
	Text = "before",
})
local _arg0 = function(item)
	return React.createElement("textlabel", {
		Key = item,
		Text = item,
	})
end
-- ▼ ReadonlyArray.map ▼
local _newValue = table.create(#items)
for _k, _v in items do
	_newValue[_k] = _arg0(_v, _k - 1, items)
end
-- ▲ ReadonlyArray.map ▲
local mixedChildren = (React.createElement("frame", nil, _exp, _newValue, unpack(elements)))
local emptyExpressions = (React.createElement("frame", nil, React.createElement("textlabel")))
local _attributes_10 = {}
local _original_2 = counter
counter += 1
_attributes_10.LayoutOrder = _original_2
local _exp_1 = React.createElement("textlabel", _attributes_10)
local _exp_2 = getName()
local _attributes_11 = {}
local _original_3 = counter
counter += 1
_attributes_11.LayoutOrder = _original_3
local childPrereqs = (React.createElement("frame", {
	Size = 1,
}, _exp_1, _exp_2, React.createElement("textlabel", _attributes_11)))
local childValue = React.createElement("frame", nil, label)
-- fragments
local emptyFragment = React.createElement(React.Fragment)
local fragment = (React.createElement(React.Fragment, nil, React.createElement("frame"), React.createElement("textlabel", {
	Text = "two",
})))
local _attributes_12 = {}
local _original_4 = counter
counter += 1
_attributes_12.LayoutOrder = _original_4
local fragmentPrereqs = (React.createElement(React.Fragment, nil, React.createElement("frame", _attributes_12)))
local keyedFragment = (React.createElement(React.Fragment, {
	Key = "keyed",
}, React.createElement("frame")))
return {
	Button = Button,
	List = List,
	Conditional = Conditional,
	empty = empty,
	host = host,
	component = component,
	componentWithChildren = componentWithChildren,
	dotted = dotted,
	nested = nested,
	booleanAttribute = booleanAttribute,
	stringAttributes = stringAttributes,
	expressionAttributes = expressionAttributes,
	namespacedAttribute = namespacedAttribute,
	namespacedTag = namespacedTag,
	underscoreTag = underscoreTag,
	elementAttribute = elementAttribute,
	attributePrereqs = attributePrereqs,
	attributeCall = attributeCall,
	spreadFirst = spreadFirst,
	spreadOnly = spreadOnly,
	spreadAfter = spreadAfter,
	spreadCall = spreadCall,
	spreadMaybe = spreadMaybe,
	spreadMaybeCall = spreadMaybeCall,
	spreadConditional = spreadConditional,
	spreadTwice = spreadTwice,
	spreadObject = spreadObject,
	keyedChildren = keyedChildren,
	arrayChild = arrayChild,
	spreadChild = spreadChild,
	mixedChildren = mixedChildren,
	emptyExpressions = emptyExpressions,
	childPrereqs = childPrereqs,
	childValue = childValue,
	emptyFragment = emptyFragment,
	fragment = fragment,
	fragmentPrereqs = fragmentPrereqs,
	keyedFragment = keyedFragment,
}
