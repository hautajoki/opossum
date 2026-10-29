--!strict
--!optimize 2
-- Collects a suite's cases and runs them into a report. Dependency-free but for expect, so it runs unchanged in the VM
-- and on Roblox.

local expect = require("./expect")

export type TestOptions = {
	only: boolean?,
	skip: (boolean | string)?,
}

export type BenchOptions = {
	only: boolean?,
	skip: (boolean | string)?,
	iterations: number?,
	samples: number?,
	beforeSample: (() -> ())?,
	afterSample: (() -> ())?,
}

-- `focus`: nil focuses on the selected `only` cases when there are any; true or false is the host's decision for the
-- whole run (a local run collects each file on its own, so it decides focus across files). `count` selects and runs
-- nothing: the report says how many cases a run would select, and how many of them have `only`.
export type RunOptions = {
	mode: ("test" | "bench")?,
	filter: string?,
	focus: boolean?,
	count: boolean?,
	onCaseComplete: (() -> ())?,
}

type ErrorReport = {
	type: string,
	source: string,
	line: number,
	message: string,
	trace: string,
}

type Hook = () -> ()

type Case = {
	kind: "test" | "bench",
	labels: { string },
	run: () -> (),
	before_each: { Hook },
	after_each: { Hook },
	only: boolean,
	skip: (boolean | string)?,
	bench: BenchOptions?,
}

type Context = {
	labels: { string },
	before_each: { Hook },
	after_each: { Hook },
}

export type FailureReport = {
	kind: "test" | "bench",
	labels: { string },
	message: string,
	source: string,
	line: number,
	trace: string,
}

export type TestReport = {
	labels: { string },
	duration_seconds: number,
}

export type SkippedReport = {
	kind: "test" | "bench",
	labels: { string },
	reason: string,
}

export type BenchReport = {
	labels: { string },
	iterations: number,
	samples: number,
	calibration_iterations: number,
	total_seconds: number,
	mean_seconds: number,
	median_seconds: number,
	min_seconds: number,
	max_seconds: number,
	margin_percent: number,
}

-- `only`: how many selected cases (after mode and filter) asked for focus; `focused`: whether the run kept only those.
export type RunReport = {
	ok: boolean,
	mode: string,
	filter: string?,
	only: number,
	focused: boolean,
	duration_seconds: number,
	discovered: number,
	selected: number,
	test_total: number,
	test_pass: number,
	test_fail: number,
	test_skip: number,
	bench_total: number,
	bench_pass: number,
	bench_fail: number,
	bench_skip: number,
	failures: { FailureReport },
	skipped: { SkippedReport },
	tests: { TestReport },
	benchmarks: { BenchReport },
}

export type Suite = {
	describe: (label: string, inner: () -> ()) -> (),
	test: (label: string, run: () -> (), options: TestOptions?) -> (),
	testEach: (values: { any }, label: string | ((any, number) -> string), run: (any, number) -> (), options: TestOptions?) -> (),
	bench: (label: string, run: () -> (), options: BenchOptions?) -> (),
	todo: (label: string, reason: string?) -> (),
	beforeEach: (hook: Hook) -> (),
	afterEach: (hook: Hook) -> (),
	cleanup: (hook: Hook) -> (),
	expect: (unknown) -> expect.Expectation,
	skip: (reason: string?) -> never,
	fail: (message: string?) -> never,
}

export type Collection = {
	cases: { Case },
	begin_case: () -> (),
	drain_cleanups: () -> { Hook },
}

local DEFAULT_SAMPLES = 12
local TARGET_SAMPLE_SECONDS = 0.005
local MAX_AUTO_ITERATIONS = 1_048_576

local function clone<T>(values: { T }): { T }
	return table.clone(values)
end

local OWN = debug.info(1, "s")

-- The xpcall handler of every case and hook. An error raised by `error()`, `assert()` or `t.fail` is reported at the
-- line that called them: C functions and this module's own frames are skipped.
local function capture_error(message: unknown): ErrorReport
	if typeof(message) == "table" then
		local tagged = message :: { type: unknown }
		if tagged.type == "spec.error" or tagged.type == "spec.skip" then return message :: ErrorReport end
	end
	local level = 2
	while true do
		local source, line = debug.info(level, "sl")
		if source == nil then break end
		if source ~= OWN and source ~= "[C]" then
			return {
				type = "spec.error",
				source = source,
				line = line,
				message = tostring(message),
				trace = debug.traceback(nil, level),
			}
		end
		level += 1
	end
	return { type = "spec.error", source = "unknown", line = 0, message = tostring(message), trace = debug.traceback(nil, 2) }
end

local function positive_integer(value: number?, name: string): number?
	if value == nil then return nil end
	if value <= 0 or value ~= math.floor(value) then error(`bench option {name} must be a positive integer`, 3) end
	return value
end

local spec = {}

function spec.collect(load: (Suite) -> ()): Collection
	local cases: { Case } = {}
	local current: Context? = nil
	local cleanups: { Hook }? = nil
	local api = {} :: any

	local function context(): Context
		return assert(current, "suite methods must be called while collecting a suite")
	end

	local function register(kind: "test" | "bench", label: string, run: () -> (), options: TestOptions?, bench: BenchOptions?)
		if label == "" then error("case labels cannot be empty", 3) end
		local active = context()
		local labels = clone(active.labels)
		table.insert(labels, label)
		table.insert(cases, {
			kind = kind,
			labels = labels,
			run = run,
			before_each = clone(active.before_each),
			after_each = clone(active.after_each),
			only = options ~= nil and options.only == true,
			skip = if options then options.skip else nil,
			bench = bench,
		})
	end

	api.describe = function(label: string, inner: () -> ())
		local outer = context()
		local labels = clone(outer.labels)
		table.insert(labels, label)
		current = {
			labels = labels,
			before_each = clone(outer.before_each),
			after_each = clone(outer.after_each),
		}
		local ok, message = xpcall(function(): ErrorReport?
			inner()
			return nil
		end, capture_error)
		current = outer
		if not ok then error(message, 0) end
	end

	api.test = function(label: string, run: () -> (), options: TestOptions?)
		register("test", label, run, options, nil)
	end

	-- One case per value, labelled `label(value, index)` or `label [index]`; `index` is 1-based.
	api.testEach = function(values: { any }, label: string | ((any, number) -> string), run: (any, number) -> (), options: TestOptions?)
		for index, value in values do
			local case_label = if type(label) == "function" then label(value, index) else `{label} [{index}]`
			api.test(case_label, function()
				run(value, index)
			end, options)
		end
	end

	api.bench = function(label: string, run: () -> (), options: BenchOptions?)
		local declared: BenchOptions = options or ({} :: BenchOptions)
		positive_integer(declared.iterations, "iterations")
		positive_integer(declared.samples, "samples")
		register("bench", label, run, declared, declared)
	end

	api.todo = function(label: string, reason: string?)
		api.test(label, function() end, { skip = reason or "todo" })
	end

	api.beforeEach = function(hook: Hook)
		table.insert(context().before_each, hook)
	end

	api.afterEach = function(hook: Hook)
		table.insert(context().after_each, hook)
	end

	api.cleanup = function(hook: Hook)
		local active = cleanups
		if active == nil then error("t.cleanup() must be called while a case is running", 2) end
		table.insert(active, hook)
	end

	api.expect = expect
	api.skip = function(reason: string?): never
		local source, line = debug.info(2, "sl")
		error({
			type = "spec.skip",
			source = if type(source) == "string" then source else "unknown",
			line = if type(line) == "number" then line else 0,
			message = reason or "skipped",
			trace = "",
		}, 0)
	end
	api.fail = function(message: string?): never
		error(message or "explicit failure", 2)
	end

	current = { labels = {}, before_each = {}, after_each = {} }
	local ok, message = xpcall(function(): ErrorReport?
		load(api :: Suite)
		return nil
	end, capture_error)
	current = nil
	if not ok then error(message, 0) end
	return {
		cases = cases,
		begin_case = function()
			if cleanups ~= nil then error("a case is already running", 2) end
			cleanups = {}
		end,
		drain_cleanups = function(): { Hook }
			local active = cleanups
			if active == nil then error("no case is running", 2) end
			cleanups = nil
			return active
		end,
	}
end

-- The median of a sorted list: the mean of the two middle values for an even count.
local function median(sorted: { number }): number
	local middle = #sorted // 2
	if #sorted % 2 == 1 then return sorted[middle + 1] end
	return (sorted[middle] + sorted[middle + 1]) / 2
end

local function invoke_hook(hook: Hook): ErrorReport?
	local ok, message = xpcall(function(): ErrorReport?
		hook()
		return nil
	end, capture_error)
	return if ok then nil else assert(message)
end

local function invoke_cleanup_hooks(hooks: { Hook }): { ErrorReport }
	local failures: { ErrorReport } = {}
	for index = #hooks, 1, -1 do
		local failure = invoke_hook(hooks[index])
		if failure ~= nil then table.insert(failures, failure) end
	end
	return failures
end

local function append_cleanup_failures(primary: ErrorReport?, failures: { ErrorReport }): ErrorReport?
	if #failures == 0 then return primary end
	local result = primary or failures[1]
	if primary == nil then table.remove(failures, 1) end
	if result.type == "spec.skip" then
		result = {
			type = "spec.error",
			source = result.source,
			line = result.line,
			message = `case skipped but cleanup failed: {result.message}`,
			trace = result.trace,
		}
	end
	for _, failure in failures do
		result.message ..= `\ncleanup failed: {failure.message}`
		if failure.trace ~= "" then
			result.trace ..= `\n{failure.trace}`
		end
	end
	return result
end

local function measure(benchmark: Case): BenchReport
	local options = benchmark.bench :: BenchOptions
	local samples = options.samples or DEFAULT_SAMPLES
	local sample_iterations: { number } = {}
	local calibration_iterations = 0
	local measured_iterations = 0

	local function before_sample()
		if options.beforeSample ~= nil then options.beforeSample() end
	end

	local function after_sample()
		if options.afterSample ~= nil then options.afterSample() end
	end

	local function run_sample(iterations: number): number
		local failure = invoke_hook(before_sample)
		local elapsed = 0
		if failure == nil then
			local started = os.clock()
			local ok, result = xpcall(function(): ErrorReport?
				for _ = 1, iterations do
					benchmark.run()
				end
				return nil
			end, capture_error)
			elapsed = os.clock() - started
			if not ok then failure = result end
		end
		local cleanup_failures: { ErrorReport } = {}
		local cleanup_failure = invoke_hook(after_sample)
		if cleanup_failure ~= nil then table.insert(cleanup_failures, cleanup_failure) end
		failure = append_cleanup_failures(failure, cleanup_failures)
		if failure ~= nil then error(failure, 0) end
		return elapsed
	end

	if options.iterations ~= nil then
		samples = math.min(samples, options.iterations)
		local base = math.floor(options.iterations / samples)
		local remainder = options.iterations % samples
		for sample = 1, samples do
			sample_iterations[sample] = base + if sample <= remainder then 1 else 0
		end
		measured_iterations = options.iterations
	else
		local iterations = 1
		while true do
			local elapsed = run_sample(iterations)
			calibration_iterations += iterations
			if elapsed >= TARGET_SAMPLE_SECONDS or iterations >= MAX_AUTO_ITERATIONS then break end
			iterations = math.min(iterations * 2, MAX_AUTO_ITERATIONS)
		end
		for sample = 1, samples do
			sample_iterations[sample] = iterations
		end
		measured_iterations = iterations * samples
	end

	local timings: { number } = table.create(samples, 0)
	local total = 0
	for sample = 1, samples do
		local iterations = sample_iterations[sample]
		local elapsed = run_sample(iterations)
		total += elapsed
		timings[sample] = elapsed / iterations
	end

	table.sort(timings)
	local mean = total / measured_iterations
	local variance = 0
	for _, timing in timings do
		variance += (timing - mean) ^ 2
	end
	if samples > 1 then
		variance /= samples - 1
	end
	local margin = if mean > 0 then 1.96 * math.sqrt(variance / samples) / mean * 100 else 0

	return {
		labels = benchmark.labels,
		iterations = measured_iterations,
		samples = samples,
		calibration_iterations = calibration_iterations,
		total_seconds = total,
		mean_seconds = mean,
		median_seconds = median(timings),
		min_seconds = timings[1],
		max_seconds = timings[#timings],
		margin_percent = margin,
	}
end

-- Selects cases by mode, then filter, then focus, and runs them. In bench mode a file's tests (its correctness
-- guards) run before any bench, and a file whose tests failed has its benches skipped instead of timed. A case's
-- first label is its suite file.
function spec.run(collection: Collection, options: RunOptions?): RunReport
	local opts: RunOptions = options or ({} :: RunOptions)
	local mode = opts.mode or "test"
	local declared_filter = opts.filter
	local filter: string? = if declared_filter ~= nil and declared_filter ~= "" then declared_filter:lower() else nil
	local selected: { Case } = {}
	local only = 0
	for _, case in collection.cases do
		-- bench mode keeps the tests of bench files: they guard the timings
		local correct_mode = mode ~= "test" or case.kind == "test"
		local correct_name = filter == nil or table.concat(case.labels, " / "):lower():find(filter, 1, true) ~= nil
		if correct_mode and correct_name then
			table.insert(selected, case)
			if case.only then
				only += 1
			end
		end
	end

	local focused = if opts.focus == nil then only > 0 else opts.focus
	if focused then
		local kept = {}
		for _, case in selected do
			if case.only then table.insert(kept, case) end
		end
		selected = kept
	end
	if mode ~= "test" then
		local ordered: { Case } = {}
		for _, case in selected do
			if case.kind == "test" then table.insert(ordered, case) end
		end
		for _, case in selected do
			if case.kind == "bench" then table.insert(ordered, case) end
		end
		selected = ordered
	end

	local report: RunReport = {
		ok = true,
		mode = mode,
		filter = opts.filter,
		only = only,
		focused = focused,
		duration_seconds = 0,
		discovered = #collection.cases,
		selected = #selected,
		test_total = 0,
		test_pass = 0,
		test_fail = 0,
		test_skip = 0,
		bench_total = 0,
		bench_pass = 0,
		bench_fail = 0,
		bench_skip = 0,
		failures = {},
		skipped = {},
		tests = {},
		benchmarks = {},
	}
	if opts.count then return report end
	local failed_files: { [string]: true } = {}
	local run_started = os.clock()

	local function skip(case: Case, reason: string)
		if case.kind == "test" then
			report.test_skip += 1
		else
			report.bench_skip += 1
		end
		table.insert(report.skipped, { kind = case.kind, labels = case.labels, reason = reason })
	end

	for _, case in selected do
		if case.kind == "test" then
			report.test_total += 1
		else
			report.bench_total += 1
		end
		if case.skip then
			skip(case, if type(case.skip) == "string" then case.skip else "skipped")
		elseif case.kind == "bench" and failed_files[case.labels[1]] then
			skip(case, "correctness guard failed")
		else
			local started = os.clock()
			collection.begin_case()
			local failure: ErrorReport? = nil
			for _, hook in case.before_each do
				failure = invoke_hook(hook)
				if failure ~= nil then break end
			end
			local outcome: BenchReport?
			if failure == nil then
				local ok, result = xpcall(function(): unknown
					if case.kind == "bench" then return measure(case) end
					case.run()
					return nil
				end, capture_error)
				if ok then
					outcome = result :: BenchReport?
				else
					failure = result :: ErrorReport
				end
			end
			local cleanup_failures = invoke_cleanup_hooks(collection.drain_cleanups())
			for _, hook_failure in invoke_cleanup_hooks(case.after_each) do
				table.insert(cleanup_failures, hook_failure)
			end
			failure = append_cleanup_failures(failure, cleanup_failures)

			if failure ~= nil and failure.type == "spec.skip" then
				skip(case, failure.message)
			elseif failure ~= nil then
				report.ok = false
				if case.kind == "test" then
					report.test_fail += 1
					failed_files[case.labels[1]] = true
				else
					report.bench_fail += 1
				end
				table.insert(report.failures, {
					kind = case.kind,
					labels = case.labels,
					message = failure.message,
					source = failure.source,
					line = failure.line,
					trace = failure.trace,
				})
			elseif case.kind == "test" then
				report.test_pass += 1
				table.insert(report.tests, { labels = case.labels, duration_seconds = os.clock() - started })
			else
				report.bench_pass += 1
				table.insert(report.benchmarks, outcome :: BenchReport)
			end
		end
		if opts.onCaseComplete ~= nil then opts.onCaseComplete() end
	end

	report.duration_seconds = os.clock() - run_started
	return report
end

return table.freeze(spec)
