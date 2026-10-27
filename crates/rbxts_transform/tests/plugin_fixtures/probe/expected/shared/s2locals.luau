-- Compiled with roblox-ts v3.0.0
local top1 = 1
local top2 = 2
local atTop = "f2 m2"
local function inFunction()
	local a = 1
	local b, c = 2, 3
	if a > 0 then
		local nested = 4
		print(nested)
	end
	return "f3 m3" .. tostring(a) .. tostring(b) .. tostring(c) .. tostring(top1) .. tostring(top2)
end
-- a call claimed inside parentheses of a `for` head
do
	local _ = "f4 m4"
	while true do
		break
	end
end
-- a claimed literal indexed or called: parenthesized
local lit = ("f4 m4").p
return {
	atTop = atTop,
	inFunction = inFunction,
	lit = lit,
}
