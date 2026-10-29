--!strict
-- Expectations. A failure is reported at the suite's line: frames of this module's own chunk are skipped, whatever
-- the host names it (a file in the VM, the one script of a Roblox bundle).

local quote = require("./quote")

local OWN = debug.info(1, "s")

export type Expectation = {
	never: Expectation,
	toBe: (expected: unknown) -> Expectation,
	toEqual: (expected: unknown) -> Expectation,
	toBeNear: (expected: number, tolerance: number?) -> Expectation,
	toBeTruthy: () -> Expectation,
	toExist: () -> Expectation,
	toHaveType: (expected: string) -> Expectation,
	toHaveKey: (expected: unknown) -> Expectation,
	toContain: (expected: unknown) -> Expectation,
	toMatch: (pattern: string) -> Expectation,
	toThrow: (pattern: string?) -> Expectation,
}

local function deep_equal(left: unknown, right: unknown, seen: { [any]: any }): boolean
	if left == right then return true end
	if type(left) ~= "table" or type(right) ~= "table" then return false end
	local left_table = left :: { [unknown]: unknown }
	local right_table = right :: { [unknown]: unknown }
	if seen[left_table] ~= nil then return seen[left_table] == right_table end
	seen[left_table] = right_table
	for key, value in left_table do
		if not deep_equal(value, right_table[key], seen) then return false end
	end
	for key in right_table do
		if left_table[key] == nil then return false end
	end
	return true
end

-- The first frame outside this module and the C functions it calls: its source, line and trace.
local function source_location(): (string, number, string)
	local level = 3
	while true do
		local source, line = debug.info(level, "sl")
		if source == nil then break end
		if source ~= OWN and source ~= "[C]" then return source, line, debug.traceback(nil, level) end
		level += 1
	end
	return "unknown", 0, debug.traceback(nil, 3)
end

local function fail(actual: unknown, assertion: string, expected: unknown, detail: string?): never
	local source, line, trace = source_location()
	local message = `expected {quote(actual)} {assertion}`
	if expected ~= nil then
		message ..= ` {quote(expected)}`
	end
	if detail ~= nil then
		message ..= `\n{detail}`
	end
	error({
		type = "spec.error",
		source = source,
		line = line,
		message = message,
		trace = trace,
	}, 0)
end

local function expect(actual: unknown): Expectation
	local function make(negated: boolean): Expectation
		local self = {} :: any
		local function check(ok: boolean, assertion: string, expected: unknown, detail: string?): Expectation
			if ok == negated then fail(actual, if negated then `not {assertion}` else assertion, expected, detail) end
			return self
		end

		self.toBe = function(expected: unknown)
			return check(actual == expected, "to be", expected, nil)
		end
		self.toEqual = function(expected: unknown)
			return check(deep_equal(actual, expected, {}), "to equal", expected, nil)
		end
		self.toBeNear = function(expected: number, tolerance: number?)
			local epsilon = tolerance or 1e-6
			if typeof(actual) ~= "number" then fail(actual, "to be a number near", expected, nil) end
			if epsilon < 0 then error("expect().toBeNear() tolerance must be non-negative", 2) end
			return check(math.abs((actual :: number) - expected) <= epsilon, "to be near", expected, `tolerance: {epsilon}`)
		end
		self.toBeTruthy = function()
			return check(actual ~= nil and actual ~= false, "to be truthy", nil, nil)
		end
		self.toExist = function()
			return check(actual ~= nil, "to exist", nil, nil)
		end
		self.toHaveType = function(expected: string)
			return check(typeof(actual) == expected, "to have type", expected, `actual type: {typeof(actual)}`)
		end
		self.toHaveKey = function(expected: unknown)
			if type(actual) ~= "table" then fail(actual, "to be a table containing key", expected, nil) end
			return check((actual :: { [unknown]: unknown })[expected] ~= nil, "to have key", expected, nil)
		end
		self.toContain = function(expected: unknown)
			if type(actual) ~= "table" then fail(actual, "to be an array containing", expected, nil) end
			return check(table.find(actual :: { unknown }, expected) ~= nil, "to contain", expected, nil)
		end
		self.toMatch = function(pattern: string)
			if type(actual) ~= "string" then fail(actual, "to be a string matching", pattern, nil) end
			return check((actual :: string):match(pattern) ~= nil, "to match", pattern, nil)
		end
		self.toThrow = function(pattern: string?)
			if type(actual) ~= "function" then fail(actual, "to be a function that throws", pattern, nil) end
			local ok, message = pcall(function(): unknown
				(actual :: () -> ())()
				return nil
			end)
			local matches = not ok and (pattern == nil or tostring(message):match(pattern) ~= nil)
			return check(matches, "to throw", pattern, if ok then "function returned successfully" else `threw: {message}`)
		end
		return self
	end

	local positive = make(false)
	local negative = make(true)
	positive.never = negative
	negative.never = positive
	return positive
end

return expect
