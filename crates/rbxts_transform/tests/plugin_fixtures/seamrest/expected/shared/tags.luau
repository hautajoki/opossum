-- Compiled with roblox-ts v3.0.0
-- @jsx createElement
-- Dotted JSX tags are accesses roblox-ts lowers itself: offered to the expression hooks and to `property_key`.
local function dotted()
	return createElement(exprWrap(tags.probeAccTag))
end
local function keyedTag()
	return createElement(keyedTags.k_Comp)
end
local function plainTag()
	return createElement(tags.plainTag)
end
return {
	dotted = dotted,
	keyedTag = keyedTag,
	plainTag = plainTag,
}
