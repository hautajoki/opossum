--!strict
-- The report's JSON encoder, the same in every host. It always writes valid JSON: invalid UTF-8 becomes U+FFFD and a
-- non-finite number null. Object keys are sorted, so the output is deterministic.

local ESCAPES: { [string]: string } = {
	['"'] = '\\"',
	["\\"] = "\\\\",
	["\b"] = "\\b",
	["\f"] = "\\f",
	["\n"] = "\\n",
	["\r"] = "\\r",
	["\t"] = "\\t",
}

local function escape_character(character: string): string
	return ESCAPES[character] or string.format("\\u%04x", string.byte(character))
end

-- `value` with every byte that does not start a valid UTF-8 sequence replaced by U+FFFD.
local function valid_utf8(value: string): string
	local parts = {}
	local from = 1
	while true do
		local length, bad = utf8.len(value, from)
		if length ~= nil then break end
		local at = bad :: number
		table.insert(parts, string.sub(value, from, at - 1))
		table.insert(parts, "\u{FFFD}")
		from = at + 1
	end
	if from == 1 then return value end
	table.insert(parts, string.sub(value, from))
	return table.concat(parts)
end

local function escape(value: string): string
	return '"' .. (string.gsub(valid_utf8(value), '[%z\1-\31\\"]', escape_character)) .. '"'
end

local function encode(value: unknown): string
	local kind = type(value)
	if value == nil then return "null" end
	if kind == "boolean" then return tostring(value) end
	if kind == "number" then
		local number = value :: number
		if number ~= number or number == math.huge or number == -math.huge then return "null" end
		return tostring(number)
	end
	if kind == "string" then return escape(value :: string) end
	if kind ~= "table" then error(`cannot encode {kind} as JSON`, 2) end

	local object = value :: { [unknown]: unknown }
	local length = #object
	local count = 0
	local array = true
	for key in object do
		count += 1
		if type(key) ~= "number" or key < 1 or key > length or key ~= math.floor(key) then array = false end
	end
	local parts = {}
	if array and count == length then
		for index = 1, length do
			parts[index] = encode(object[index])
		end
		return "[" .. table.concat(parts, ",") .. "]"
	end
	for key, item in object do
		if type(key) ~= "string" then error("JSON object keys must be strings", 2) end
		table.insert(parts, `{escape(key)}:{encode(item)}`)
	end
	table.sort(parts)
	return "{" .. table.concat(parts, ",") .. "}"
end

return encode
