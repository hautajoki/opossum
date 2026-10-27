-- Compiled with roblox-ts v3.0.0
-- plugin diagnostics are kept once in the build: one identical to roblox-ts's own warning at the same node, and
-- one at a node in another file, which two files report
local function check()
	if count ~= 0 and count == count and count then
		return 1
	end
	return nil
end
return {
	check = check,
}
