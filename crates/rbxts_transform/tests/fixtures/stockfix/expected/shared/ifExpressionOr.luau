-- Compiled with roblox-ts v3.0.0
-- An if-expression on the left of `or`: `$range`'s fallback for a nil step. roblox-ts rendered
-- `if flag then steps[6] else 2 or 1`, where the `or 1` joins the `else` branch, so a nil step reached the loop.
local steps = {}
local function rangeStep(flag)
	local log = {}
	for i = 1, 3, (if flag then steps[6] else 2) or 1 do
		table.insert(log, `{i}`)
	end
	return log
end
return {
	rangeStep = rangeStep,
}
