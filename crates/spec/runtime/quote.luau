--!strict
-- Deterministic failure-value formatting: table entries sorted, a table inside itself printed as <cycle>.

local function format(value: unknown, indent: number, open: { [unknown]: true }): string
	local kind = typeof(value)
	if kind == "string" then return string.format("%q", value :: string) end
	if kind ~= "table" then return tostring(value) end
	if open[value] then return "<cycle>" end

	open[value] = true
	local inner = indent + 1
	local lines = {}
	for key, item in value :: { [unknown]: unknown } do
		table.insert(lines, `{string.rep("\t", inner)}[{format(key, inner, open)}] = {format(item, inner, open)}`)
	end
	open[value] = nil
	table.sort(lines)
	if #lines == 0 then return "{}" end
	return "{\n" .. table.concat(lines, ",\n") .. "\n" .. string.rep("\t", indent) .. "}"
end

local function quote(value: unknown): string
	return format(value, 0, {})
end

return quote
