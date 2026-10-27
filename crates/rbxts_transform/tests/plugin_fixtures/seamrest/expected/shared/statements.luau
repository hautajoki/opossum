-- Compiled with roblox-ts v3.0.0
-- Statements roblox-ts lowers outside its statement dispatch, offered to the statement hooks: a block body (`if`,
-- loops, `case`, a static block), an `else if`, and a `declare` statement (which roblox-ts emits nothing for).
print("probe: a declare statement")
local function blockBody(flag)
	if flag then
		print("probe: a block of 2 statements")
	end
	return 2
end
local function elseIf(a)
	if a then
		return "a"
	else
		print("probe: an if statement")
	end
	return "c"
end
local function loopBody()
	local total = 0
	for _, x in { 1, 2 } do
		print("probe: a block of 2 statements")
	end
	return total
end
local function caseBody(n)
	repeat
		local _fallthrough = false
		if n == 1 then
			print("probe: a block of 2 statements")
		end
		return "other"
	until true
end
local WithStatic
do
	WithStatic = setmetatable({}, {
		__tostring = function()
			return "WithStatic"
		end,
	})
	WithStatic.__index = WithStatic
	function WithStatic.new(...)
		local self = setmetatable({}, WithStatic)
		return self:constructor(...) or self
	end
	function WithStatic:constructor()
	end
	WithStatic.value = 1
	print("probe: a block of 2 statements")
end
return {
	blockBody = blockBody,
	elseIf = elseIf,
	loopBody = loopBody,
	caseBody = caseBody,
	WithStatic = WithStatic,
}
