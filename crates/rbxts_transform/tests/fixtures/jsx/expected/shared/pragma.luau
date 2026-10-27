-- Compiled with roblox-ts v3.0.0
-- @jsx h
-- @jsxFrag Frag
-- Hand-written JSX fixture: a file's own JSX factory and fragment factory from pragmas.
local function h(tag, props, ...)
	local children = { ... }
	return {
		tag = tag,
		props = props,
		children = children,
	}
end
local Frag = "Frag"
return function()
	local element = h("text", {
		value = 1,
	}, "text")
	local fragment = (h(Frag, nil, h("text")))
	return { element, fragment }
end
