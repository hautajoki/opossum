--!strict
-- One run of the suites the host lists, in every host. The host passes the list and how to load a suite (a file in
-- the VM, an instance path on Roblox); each suite's label, its source file in the project, is the first label of its
-- cases, so a filter selects the same cases everywhere.

local json = require("./json")
local spec = require("./spec")

-- `path`: the suite's instance names from `game` (Roblox only; in the VM the label is the file).
export type Suite = { label: string, path: { string }? }

export type Request = {
	suites: { Suite },
	mode: "test" | "bench",
	filter: string?,
	focus: boolean?,
	count: boolean?,
}

-- On Roblox a run yields every 16 cases or every 0.1 s, so a long run does not hold the engine (or trip its script
-- timeout) between cases; a single long case still can.
local CASES_PER_YIELD = 16
local MAX_SECONDS_WITHOUT_YIELD = 0.1

local runner = {}

-- Collects every suite, runs the selection and returns the report as JSON. A suite that fails to load or to register
-- becomes a failing case; the others still run. `wait` yields to the engine (Roblox's `task.wait`).
function runner.run(request: Request, load: (Suite) -> unknown, wait: (() -> ())?): string
	local collection = spec.collect(function(t)
		for _, suite in request.suites do
			t.describe(suite.label, function()
				local loaded, exported = pcall(load, suite)
				if not loaded then
					t.test("loads suite", function()
						error(exported, 0)
					end)
					return
				end
				local run: unknown = exported
				if type(exported) == "table" then run = (exported :: { default: unknown }).default end
				if type(run) ~= "function" then
					t.test("loads suite", function()
						error("suite module must return a function", 0)
					end)
					return
				end
				local registered, message = pcall(run :: (spec.Suite) -> (), t)
				if not registered then t.test("registers suite", function()
					error(message, 0)
				end) end
			end)
		end
	end)

	local cases_since_yield = 0
	local last_yield = os.clock()
	local function yield()
		cases_since_yield += 1
		if cases_since_yield < CASES_PER_YIELD and os.clock() - last_yield < MAX_SECONDS_WITHOUT_YIELD then return end
		(wait :: () -> ())()
		cases_since_yield = 0
		last_yield = os.clock()
	end

	return json(spec.run(collection, {
		mode = request.mode,
		filter = request.filter,
		focus = request.focus,
		count = request.count,
		onCaseComplete = if wait ~= nil then yield else nil,
	}))
end

return table.freeze(runner)
