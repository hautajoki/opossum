-- Compiled with roblox-ts v3.0.0
-- Hand-written JSX fixture: a small React-like factory, imported as `React` like `@rbxts/react` (`export =`).
local React = {}
do
	local _container = React
	local function createElement(tag, props, ...)
		local children = { ... }
		return {
			tag = tag,
			props = props,
			children = children,
		}
	end
	_container.createElement = createElement
	local function Fragment(props)
		return {
			tag = "Fragment",
			props = props,
			children = {},
		}
	end
	_container.Fragment = Fragment
end
return React
