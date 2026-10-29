--!strict
-- The spec runtime's own cases, run through the VM by `cargo test` (local.rs). Each builds an inner collection from
-- the runtime's files and checks the report it gives.

local json = require("../runtime/json")
local spec = require("../runtime/spec")

local HERE = debug.info(1, "s")

return function(t: spec.Suite)
	t.describe("runtime", function()
		t.test("runs correctness guards before benchmark timings", function()
			local order: { string } = {}
			local collection = spec.collect(function(inner)
				inner.bench("timing", function()
					table.insert(order, "bench")
				end, { iterations = 1, samples = 1 })
				inner.test("guard", function()
					table.insert(order, "test")
				end)
			end)

			local report = spec.run(collection, { mode = "bench" })
			assert(report.ok)
			t.expect(table.concat(order, ",")).toBe("test,bench")
		end)

		t.test("skips a file's benches once its tests failed", function()
			local timed = false
			local collection = spec.collect(function(inner)
				inner.describe("broken", function()
					inner.bench("timing", function()
						timed = true
					end, { iterations = 1, samples = 1 })
					inner.test("guard", function()
						error("wrong")
					end)
				end)
				inner.describe("sound", function()
					inner.bench("timing", function() end, { iterations = 1, samples = 1 })
				end)
			end)

			local report = spec.run(collection, { mode = "bench" })
			assert(not timed)
			t.expect({ report.test_fail, report.bench_skip, report.bench_pass }).toEqual { 1, 1, 1 }
			t.expect(report.skipped[1].labels).toEqual { "broken", "timing" }
			t.expect(report.skipped[1].reason).toBe("correctness guard failed")
		end)

		t.test("runs every cleanup once in LIFO order after a failure", function()
			local order: { string } = {}
			local collection = spec.collect(function(inner)
				inner.afterEach(function()
					table.insert(order, "after-first")
				end)
				inner.afterEach(function()
					table.insert(order, "after-second")
					error("after exploded")
				end)
				inner.test("failure", function()
					table.insert(order, "body")
					inner.cleanup(function()
						table.insert(order, "cleanup-first")
					end)
					inner.cleanup(function()
						table.insert(order, "cleanup-second")
						error("cleanup exploded")
					end)
					error("body exploded")
				end)
			end)

			local report = spec.run(collection, { mode = "test" })
			assert(not report.ok)
			t.expect(table.concat(order, ",")).toBe("body,cleanup-second,cleanup-first,after-second,after-first")
			local message = report.failures[1].message
			t.expect(message).toMatch("body exploded")
			t.expect(message).toMatch("cleanup exploded")
			t.expect(message).toMatch("after exploded")
		end)

		t.test("runs cleanup after a successful body", function()
			local cleaned = false
			local collection = spec.collect(function(inner)
				inner.test("success", function()
					inner.cleanup(function()
						cleaned = true
					end)
				end)
			end)

			assert(spec.run(collection, { mode = "test" }).ok)
			assert(cleaned)
		end)

		t.test("runs benchmark sample cleanup after timing failure", function()
			local before_count = 0
			local after_count = 0
			local collection = spec.collect(function(inner)
				inner.bench("failure", function()
					error("timing exploded")
				end, {
					iterations = 1,
					samples = 1,
					beforeSample = function()
						before_count += 1
					end,
					afterSample = function()
						after_count += 1
						error("sample cleanup exploded")
					end,
				})
			end)

			local report = spec.run(collection, { mode = "bench" })
			assert(not report.ok)
			t.expect({ before_count, after_count }).toEqual { 1, 1 }
			t.expect(report.failures[1].message).toMatch("timing exploded")
			t.expect(report.failures[1].message).toMatch("sample cleanup exploded")
		end)

		t.test("reports focus and empty selections to the host", function()
			local collection = spec.collect(function(inner)
				inner.test("ordinary", function() end)
				inner.test("focused", function() end, { only = true })
			end)

			local focused = spec.run(collection, { mode = "test" })
			assert(focused.ok and focused.focused)
			t.expect({ focused.only, focused.selected }).toEqual { 1, 1 }

			-- the host decides: a local run's first pass only counts the `only` cases
			local unfocused = spec.run(collection, { mode = "test", focus = false })
			assert(not unfocused.focused)
			t.expect({ unfocused.only, unfocused.selected }).toEqual { 1, 2 }

			-- an `only` case the filter drops focuses nothing
			local filtered = spec.run(collection, { mode = "test", filter = "ORDINARY" })
			assert(not filtered.focused)
			t.expect({ filtered.only, filtered.selected }).toEqual { 0, 1 }

			local empty = spec.run(collection, { mode = "test", filter = "absent" })
			assert(empty.ok and not empty.focused)
			t.expect(empty.selected).toBe(0)
		end)

		t.test("passes testEach options to every case", function()
			local seen: { number } = {}
			local collection = spec.collect(function(inner)
				inner.testEach({ "a", "b" }, "value", function(_, index)
					table.insert(seen, index)
				end)
				inner.testEach({ "c" }, function(value, index)
					return `{value}{index}`
				end, function() end, { skip = "later" })
			end)

			local report = spec.run(collection, { mode = "test" })
			t.expect(seen).toEqual { 1, 2 }
			t.expect(report.tests[2].labels).toEqual { "value [2]" }
			t.expect(report.skipped[1]).toEqual { kind = "test", labels = { "c1" }, reason = "later" }
		end)

		t.test("rejects cleanup registration outside a running case", function()
			local captured: spec.Suite? = nil
			spec.collect(function(inner)
				captured = inner
			end)
			t.expect(function()
				(captured :: spec.Suite).cleanup(function() end)
			end).toThrow("while a case is running")
		end)

		t.test("reports an error at the line that raised it", function()
			local lines: { number } = {}
			local collection = spec.collect(function(inner)
				-- each statement pair is on one line, so `debug.info` names the line of the raise
				-- stylua: ignore start
				inner.test("error", function() lines[1] = debug.info(1, "l"); error("boom") end)
				inner.test("assert", function() lines[2] = debug.info(1, "l"); assert(false, "falsy") end)
				inner.test("fail", function() lines[3] = debug.info(1, "l"); inner.fail("failed") end)
				inner.test("expect", function() lines[4] = debug.info(1, "l"); inner.expect(1).toBe(2) end)
				-- stylua: ignore end
			end)

			local report = spec.run(collection, { mode = "test" })
			t.expect(#report.failures).toBe(4)
			for index, failure in report.failures do
				t.expect({ failure.source, failure.line }).toEqual { HERE, lines[index] }
			end
		end)

		t.test("quotes a cyclic value", function()
			local cyclic = {}
			cyclic.self = cyclic
			local collection = spec.collect(function(inner)
				inner.test("cycle", function()
					inner.expect(cyclic).toEqual {}
				end)
			end)

			local message = spec.run(collection, { mode = "test" }).failures[1].message
			t.expect(message).toMatch('%["self"%] = <cycle>')
		end)

		t.test("encodes any label as valid JSON", function()
			local collection = spec.collect(function(inner)
				inner.test("bad \255 byte", function()
					error("also \255")
				end)
			end)

			local encoded = json(spec.run(collection, { mode = "test" }))
			t.expect((encoded:find("\255", 1, true))).never.toExist()
			t.expect((encoded:find("bad \u{FFFD} byte", 1, true))).toExist()
			t.expect(json { 0 / 0, math.huge, -math.huge, 0.5 }).toBe("[null,null,null,0.5]")
		end)

		t.test("reports the true median of an even sample count", function()
			local sample = 0
			local collection = spec.collect(function(inner)
				inner.bench("uneven", function()
					sample += 1
					if sample == 1 then
						local started = os.clock()
						repeat
						until os.clock() - started > 0.002
					end
				end, { iterations = 2, samples = 2 })
			end)

			local bench = spec.run(collection, { mode = "bench" }).benchmarks[1]
			assert(bench.max_seconds > bench.min_seconds * 2)
			t.expect(bench.median_seconds).toBeNear((bench.min_seconds + bench.max_seconds) / 2, 1e-12)
		end)
	end)
end
