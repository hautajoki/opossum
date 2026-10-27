-- Compiled with roblox-ts v3.0.0
-- roblox-ts's tests/src/tests/loop.spec.ts
local function difference(set1, set2)
	local result = {}
	for value in set1 do
		if not (set2[value] ~= nil) then
			result[value] = true
		end
	end
	for value in set2 do
		if not (set1[value] ~= nil) then
			result[value] = true
		end
	end
	return result
end
return function()
	it("should retain loop finalizers around empty blocks and branches", function()
		local callbacks = {}
		local limit = 3
		do
			local _i = 0
			local _shouldIncrement = false
			while true do
				local i = _i
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < limit) then
					break
				end
				if i == 1 then
				end
				do
				end
				table.insert(callbacks, function()
					return i
				end)
				_i = i
			end
		end
		local _arg0 = function(callback)
			return callback()
		end
		-- ▼ ReadonlyArray.map ▼
		local _newValue = table.create(#callbacks)
		for _k, _v in callbacks do
			_newValue[_k] = _arg0(_v, _k - 1, callbacks)
		end
		-- ▲ ReadonlyArray.map ▲
		expect(table.concat(_newValue, ",")).to.equal("0,1,2")
	end)
	it("should iterate inferred tuples through declarations and assignment targets", function()
		local index = 0
		local function advance()
			index += 1
			assert(index <= 3, "iterator was called after its first return value became nil")
			return if index <= 2 then index else nil, index * 10
		end
		local iterator = advance
		local total = 0
		for _element, _element_1 in iterator do
			local pair = { _element, _element_1 }
			total += pair[2]
		end
		expect(total).to.equal(30)
		index = 0
		local pair
		local _iterator = iterator
		while true do
			local _v = { _iterator() }
			if _v[1] == nil then
				break
			end
			pair = _v
			total += pair[2]
		end
		expect(total).to.equal(60)
	end)
	it("should run loop increments around empty branches and blocks", function()
		local limit = 3
		local visits = 0
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < limit) then
					break
				end
				if i == 0 then
				else
					visits += 1
				end
				do
				end
			end
		end
		expect(visits).to.equal(2)
	end)
	it("should finalize captured loop variables before nested continues", function()
		local values = {}
		local visited = {}
		local limit = 4
		do
			local _i = 0
			local _shouldIncrement = false
			while true do
				local i = _i
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < limit) then
					break
				end
				table.insert(values, function()
					return i
				end)
				if i == 0 then
					table.insert(visited, i)
					_i = i
					continue
				elseif i == 1 then
					_i = i
					continue
				else
					do
						if i == 2 then
							_i = i
							continue
						end
						table.insert(visited, i)
						_i = i
						continue
					end
				end
				_i = i
			end
		end
		local _arg0 = function(read)
			return read()
		end
		-- ▼ ReadonlyArray.map ▼
		local _newValue = table.create(#values)
		for _k, _v in values do
			_newValue[_k] = _arg0(_v, _k - 1, values)
		end
		-- ▲ ReadonlyArray.map ▲
		expect(table.concat(_newValue, ",")).to.equal("0,1,2,3")
		expect(table.concat(visited, ",")).to.equal("0,3")
	end)
	it("should retain the initializer binding captured by a loop initializer", function()
		local initial = {}
		local iterations = {}
		do
			local _iCopy = 0
			local read = function()
				return _iCopy
			end
			local _i = _iCopy
			local _shouldIncrement = false
			while true do
				local i = _i
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < 3) then
					break
				end
				table.insert(initial, read)
				table.insert(iterations, function()
					return i
				end)
				_i = i
			end
		end
		local _arg0 = function(read)
			return read()
		end
		-- ▼ ReadonlyArray.map ▼
		local _newValue = table.create(#initial)
		for _k, _v in initial do
			_newValue[_k] = _arg0(_v, _k - 1, initial)
		end
		-- ▲ ReadonlyArray.map ▲
		expect(table.concat(_newValue, ",")).to.equal("0,0,0")
		local _arg0_1 = function(read)
			return read()
		end
		-- ▼ ReadonlyArray.map ▼
		local _newValue_1 = table.create(#iterations)
		for _k, _v in iterations do
			_newValue_1[_k] = _arg0_1(_v, _k - 1, iterations)
		end
		-- ▲ ReadonlyArray.map ▲
		expect(table.concat(_newValue_1, ",")).to.equal("0,1,2")
	end)
	it("should collect nested loop bindings and omit skipped elements", function()
		local values = {}
		do
			local _binding, _, _binding_1 = {
				value = 0,
			}, 100, { 2 }
			local value = _binding.value
			local other = _binding_1[1]
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					value += 1
				else
					_shouldIncrement = true
				end
				if not (value < 2) then
					break
				end
				table.insert(values, value + other)
			end
		end
		expect(table.concat(values, ",")).to.equal("2,3")
	end)
	it("should retain loops with missing initializers or incrementors", function()
		local values = {}
		do
			local _i
			local _shouldIncrement = false
			while true do
				local i = _i
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				i = #values
				if not (i < 2) then
					break
				end
				table.insert(values, i)
				_i = i
			end
		end
		do
			local _i = 2
			while true do
				local i = _i
				if not (i < 4) then
					break
				end
				local _original = i
				i += 1
				table.insert(values, _original)
				_i = i
			end
		end
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i > 10) then
					break
				end
				table.insert(values, 100)
			end
		end
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i ~= 2) then
					break
				end
				table.insert(values, i)
			end
		end
		expect(table.concat(values, ",")).to.equal("0,1,2,3,0,1")
	end)
	it("should evaluate a negated do-while condition after each iteration", function()
		local iterations = 0
		local checks = 0
		local function finished()
			checks += 1
			return if iterations == 3 then "done" else ""
		end
		repeat
			do
				iterations += 1
			end
			local _value = finished()
		until _value ~= "" and _value
		expect(iterations).to.equal(3)
		expect(checks).to.equal(3)
	end)
	it("should optimize local constant bounds while retaining captures and continue", function()
		local MAX_SLOTS = 10
		local slots = {}
		local callbacks = {}
		for slot = 0, MAX_SLOTS - 1 do
			table.insert(callbacks, function()
				return slot
			end)
			if slot == 2 then
				continue
			end
			table.insert(slots, slot)
		end
		expect(table.concat(slots, ",")).to.equal("0,1,3,4,5,6,7,8,9")
		local _arg0 = function(read)
			return read()
		end
		-- ▼ ReadonlyArray.map ▼
		local _newValue = table.create(#callbacks)
		for _k, _v in callbacks do
			_newValue[_k] = _arg0(_v, _k - 1, callbacks)
		end
		-- ▲ ReadonlyArray.map ▲
		expect(table.concat(_newValue, ",")).to.equal("0,1,2,3,4,5,6,7,8,9")
	end)
	it("should support inclusive and descending local constant bounds", function()
		local start = 3
		local limit = -2
		local ascending = {}
		for i = limit, start, 2 do
			table.insert(ascending, i)
		end
		local descending = {}
		for i = start, limit + 1, -2 do
			table.insert(descending, i)
		end
		expect(table.concat(ascending, ",")).to.equal("-2,0,2")
		expect(table.concat(descending, ",")).to.equal("3,1,-1")
	end)
	it("should retain writes to induction variables with constant bounds", function()
		local limit = 10
		local values = {}
		do
			local _i = 0
			local _shouldIncrement = false
			while true do
				local i = _i
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < limit) then
					break
				end
				table.insert(values, i)
				i += 2
				_i = i
			end
		end
		expect(table.concat(values, ",")).to.equal("0,3,6,9")
	end)
	it("should reevaluate mutable identifier bounds", function()
		local limit = 4
		local values = {}
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < limit) then
					break
				end
				table.insert(values, i)
				limit -= 1
			end
		end
		expect(table.concat(values, ",")).to.equal("0,1")
	end)
	it("should normalize loop parentheses and erased type wrappers", function()
		local limit = 3
		local values = {}
		for i = 0, limit - 1 do
			table.insert(values, i)
		end
		expect(table.concat(values, ",")).to.equal("0,1,2")
	end)
	it("should retain constant aliases and steps in normalized loops", function()
		local initial = 1
		local start = initial
		local limit = start + 2 * 3
		local step = 2
		local ascending = {}
		for i = start, limit - 1, step do
			table.insert(ascending, i)
		end
		local descending = {}
		for i = limit, start, -step do
			table.insert(descending, i)
		end
		expect(table.concat(ascending, ",")).to.equal("1,3,5")
		expect(table.concat(descending, ",")).to.equal("7,5,3,1")
	end)
	it("should evaluate constant initializers once before loops", function()
		local calls = 0
		local function getLimit()
			calls += 1
			return 3
		end
		local function getStep()
			calls += 1
			return 1
		end
		local limit = getLimit()
		local step = getStep()
		local values = {}
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += step
				else
					_shouldIncrement = true
				end
				if not (i < limit) then
					break
				end
				table.insert(values, i)
			end
		end
		expect(table.concat(values, ",")).to.equal("0,1,2")
		expect(calls).to.equal(2)
	end)
	it("should preserve fractional bounds copied from literal-typed properties", function()
		local source = {
			limit = 3,
		}
		local alias = source
		alias.limit = 2.5
		local function getLimit()
			return source.limit
		end
		local computed = getLimit()
		local destructured = source.limit
		local element = source.limit
		local values = {}
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < computed) then
					break
				end
				table.insert(values, i)
			end
		end
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < destructured) then
					break
				end
				table.insert(values, i)
			end
		end
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < element) then
					break
				end
				table.insert(values, i)
			end
		end
		expect(table.concat(values, ",")).to.equal("0,1,2,0,1,2,0,1,2")
	end)
	it("should preserve fractional starts copied from literal-typed properties", function()
		local source = {
			start = 0,
		}
		local alias = source
		alias.start = 0.5
		local start = source.start
		local values = {}
		do
			local i = start
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < 3) then
					break
				end
				table.insert(values, i)
			end
		end
		expect(table.concat(values, ",")).to.equal("0.5,1.5,2.5")
	end)
	it("should preserve step signs copied from literal-typed properties", function()
		local source = {
			step = 1,
		}
		local alias = source
		alias.step = -1
		local step = source.step
		local values = {}
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += step
				else
					_shouldIncrement = true
				end
				if not (i < 3) then
					break
				end
				table.insert(values, i)
				if #values == 3 then
					break
				end
			end
		end
		expect(table.concat(values, ",")).to.equal("0,-1,-2")
	end)
	it("should use copied integer constants from destructuring", function()
		local calls = 0
		local function getOptions()
			calls += 1
			return {
				step = 1,
			}
		end
		local start, _, limit = 0, 99, 3
		local step = getOptions().step
		local values = {}
		do
			local i = start
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += step
				else
					_shouldIncrement = true
				end
				if not (i < limit) then
					break
				end
				table.insert(values, i)
			end
		end
		expect(table.concat(values, ",")).to.equal("0,1,2")
		expect(calls).to.equal(1)
	end)
	it("should support negated constants and integer arithmetic in loops", function()
		local limit = 3
		local step = -1
		local descending = {}
		for i = 0, -limit + 1, step do
			table.insert(descending, i)
		end
		local ascending = {}
		for i = 0, 6 / 2 + 1, 2 ^ 1 do
			table.insert(ascending, i)
		end
		expect(table.concat(descending, ",")).to.equal("0,-1,-2")
		expect(table.concat(ascending, ",")).to.equal("0,2,4")
	end)
	it("should preserve negative zero through a constant loop initializer", function()
		local start = -0
		local limit = 0
		local step = 1
		for i = start, limit, step do
			expect(1 / i).to.equal(-math.huge)
		end
	end)
	it("should retain effectful arithmetic conditions and step calls", function()
		local checks = 0
		local increments = 0
		local function bound()
			checks += 1
			return 2
		end
		local function step()
			increments += 1
			return 1
		end
		local values = {}
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += step()
				else
					_shouldIncrement = true
				end
				if not (i < bound() + 1) then
					break
				end
				table.insert(values, i)
			end
		end
		expect(table.concat(values, ",")).to.equal("0,1,2")
		expect(checks).to.equal(4)
		expect(increments).to.equal(3)
	end)
	it("should evaluate dynamic range negations once", function()
		local calls = 0
		local function step()
			calls += 1
			return 1
		end
		local values = {}
		for i = 3, 1, (-step()) do
			table.insert(values, i)
		end
		expect(table.concat(values, ",")).to.equal("3,2,1")
		expect(calls).to.equal(1)
	end)
	it("should retain assignment updates that are not constant steps", function()
		local assigned = {}
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i = 1
				else
					_shouldIncrement = true
				end
				if not (i < 2) then
					break
				end
				table.insert(assigned, i)
				if #assigned == 3 then
					break
				end
			end
		end
		local multiplied = {}
		do
			local i = 1
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i = i * 2
				else
					_shouldIncrement = true
				end
				if not (i < 8) then
					break
				end
				table.insert(multiplied, i)
			end
		end
		expect(table.concat(assigned, ",")).to.equal("0,1,1")
		expect(table.concat(multiplied, ",")).to.equal("1,2,4")
	end)
	it("should reevaluate nonbinary loop conditions", function()
		local checks = 0
		local function keepGoing()
			checks += 1
			return checks <= 3
		end
		local values = {}
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not keepGoing() then
					break
				end
				table.insert(values, i)
			end
		end
		expect(table.concat(values, ",")).to.equal("0,1,2")
		expect(checks).to.equal(4)
	end)
	it("should support dynamic and missing destructured loop bounds", function()
		local bounds = { 3, 4 }
		local dynamic = bounds[1]
		local _binding = {}
		local missing = _binding[1]
		if missing == nil then
			missing = 2
		end
		local _array = {}
		local _length = #_array
		table.move(bounds, 1, #bounds, _length + 1, _array)
		local spread = _array[1]
		local values = {}
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < dynamic) then
					break
				end
				table.insert(values, i)
			end
		end
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < missing) then
					break
				end
				table.insert(values, i)
			end
		end
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < spread) then
					break
				end
				table.insert(values, i)
			end
		end
		expect(table.concat(values, ",")).to.equal("0,1,2,0,1,0,1,2")
	end)
	it("should reevaluate negated mutable loop bounds", function()
		local limit = 3
		local values = {}
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i -= 1
				else
					_shouldIncrement = true
				end
				if not (i > -limit) then
					break
				end
				limit -= 1
				table.insert(values, i)
			end
		end
		expect(table.concat(values, ",")).to.equal("0,-1")
	end)
	it("should not read an uninitialized constant step before the increment", function()
		local visited = 0
		local step
		local function run()
			do
				local i = 1
				local _shouldIncrement = false
				while true do
					if _shouldIncrement then
						i += step
					else
						_shouldIncrement = true
					end
					if not (i < 0) then
						break
					end
					visited += 1
				end
			end
			do
				local i = 0
				local _shouldIncrement = false
				while true do
					if _shouldIncrement then
						i += step
					else
						_shouldIncrement = true
					end
					if not (i < 1) then
						break
					end
					visited += 1
					break
				end
			end
		end
		run()
		step = 1
		expect(visited).to.equal(1)
	end)
	it("should support numeric separators in loop bounds and steps", function()
		local ascending = {}
		for i = 0x0_0, 29, 10 do
			table.insert(ascending, i)
		end
		expect(table.concat(ascending, ",")).to.equal("0,10,20")
		local descending = {}
		for i = 3_0, 1, -10 do
			table.insert(descending, i)
		end
		expect(table.concat(descending, ",")).to.equal("30,20,10")
	end)
	it("should preserve negative zero in a loop initializer", function()
		for i = -0, 0 do
			expect(1 / i).to.equal(-math.huge)
		end
	end)
	it("should support breaking from a loop with a zero step", function()
		local iterations = 0
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 0
				else
					_shouldIncrement = true
				end
				if not (i < 1) then
					break
				end
				iterations += 1
				break
			end
		end
		expect(iterations).to.equal(1)
	end)
	it("should reevaluate a changing array-size loop bound", function()
		local values = { 1, 2, 3, 4 }
		local iterations = 0
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < #values) then
					break
				end
				values[#values] = nil
				iterations += 1
			end
		end
		expect(iterations).to.equal(2)
	end)
	it("should reevaluate calls with a numeric literal return type in loop conditions", function()
		local calls = 0
		local function limit()
			calls += 1
			return 2
		end
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < limit()) then
					break
				end
			end
		end
		expect(calls).to.equal(3)
	end)
	it("should preserve loop conditions that compare a different variable", function()
		local condition = 2
		local iterations = 0
		do
			local i = 0
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (condition < 3) then
					break
				end
				condition += 1
				iterations += 1
			end
		end
		expect(iterations).to.equal(1)
	end)
	it("should preserve loop incrementors that decrement a different variable", function()
		local remaining = 3
		do
			local i = 3
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					remaining -= 1
				else
					_shouldIncrement = true
				end
				if not (i > 0) then
					break
				end
				if remaining == 1 then
					break
				end
			end
		end
		expect(remaining).to.equal(1)
	end)
	it("should preserve destructuring writes to the loop variable", function()
		local iterations = 0
		do
			local _i = 0
			local _shouldIncrement = false
			while true do
				local i = _i
				if _shouldIncrement then
					i += 1
				else
					_shouldIncrement = true
				end
				if not (i < 3) then
					break
				end
				i = 5
				iterations += 1
				_i = i
			end
		end
		expect(iterations).to.equal(1)
	end)
	it("should support numeric for loops", function()
		local hit = {}
		local sum = 10
		for i = 0, 9 do
			hit[i] = true
			sum -= 1
		end
		expect(sum).to.equal(0)
		expect(hit[0] ~= nil).to.equal(true)
		expect(hit[1] ~= nil).to.equal(true)
		expect(hit[2] ~= nil).to.equal(true)
		expect(hit[3] ~= nil).to.equal(true)
		expect(hit[4] ~= nil).to.equal(true)
		expect(hit[5] ~= nil).to.equal(true)
		expect(hit[6] ~= nil).to.equal(true)
		expect(hit[7] ~= nil).to.equal(true)
		expect(hit[8] ~= nil).to.equal(true)
		expect(hit[9] ~= nil).to.equal(true)
	end)
	it("should support optimized simple loops #1", function()
		local hit = {}
		local n = 0
		for i = 1, 3 do
			hit[i] = true
			n += 1
		end
		expect(n).to.equal(3)
		expect(hit[1] ~= nil).to.equal(true)
		expect(hit[2] ~= nil).to.equal(true)
		expect(hit[3] ~= nil).to.equal(true)
	end)
	it("should support optimized simple loops #2", function()
		local hit = {}
		local n = 0
		for i = 3, 1, -1 do
			hit[i] = true
			n += 1
		end
		expect(n).to.equal(3)
		expect(hit[1] ~= nil).to.equal(true)
		expect(hit[2] ~= nil).to.equal(true)
		expect(hit[3] ~= nil).to.equal(true)
	end)
	it("should support optimized simple loops #3", function()
		local hit = {}
		local n = 0
		for i = 1, 3 do
			hit[i] = true
			n += 1
		end
		expect(n).to.equal(3)
		expect(hit[1] ~= nil).to.equal(true)
		expect(hit[2] ~= nil).to.equal(true)
		expect(hit[3] ~= nil).to.equal(true)
	end)
	it("should support optimized simple loops #4", function()
		local hit = {}
		local n = 0
		for i = 3, 1, -1 do
			hit[i] = true
			n += 1
		end
		expect(n).to.equal(3)
		expect(hit[1] ~= nil).to.equal(true)
		expect(hit[2] ~= nil).to.equal(true)
		expect(hit[3] ~= nil).to.equal(true)
	end)
	it("should support optimized simple loops #5", function()
		local hit = {}
		local n = 0
		for i = 1, 3 do
			hit[i] = true
			n += 1
		end
		expect(n).to.equal(3)
		expect(hit[1] ~= nil).to.equal(true)
		expect(hit[2] ~= nil).to.equal(true)
		expect(hit[3] ~= nil).to.equal(true)
	end)
	it("should support optimized simple loops #7", function()
		local hit = {}
		local limit = 1
		local n = 0
		do
			local i = 3
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i -= 1
				else
					_shouldIncrement = true
				end
				if not (i <= limit) then
					break
				end
				hit[i] = true
				n += 1
			end
		end
		expect(n).to.equal(0)
		expect(hit[1] ~= nil).to.equal(false)
		expect(hit[2] ~= nil).to.equal(false)
		expect(hit[3] ~= nil).to.equal(false)
		for i = 3, limit, -1 do
			hit[i] = true
			n += 1
		end
		expect(n).to.equal(3)
		expect(hit[1] ~= nil).to.equal(true)
		expect(hit[2] ~= nil).to.equal(true)
		expect(hit[3] ~= nil).to.equal(true)
	end)
	it("should support optimized simple loops #8", function()
		local hit = {}
		local n = 0
		do
			local i = 3
			local _shouldIncrement = false
			while true do
				if _shouldIncrement then
					i -= 1
				else
					_shouldIncrement = true
				end
				if not (i <= 1) then
					break
				end
				hit[i] = true
				n += 1
			end
		end
		expect(n).to.equal(0)
		expect(hit[1] ~= nil).to.equal(false)
		expect(hit[2] ~= nil).to.equal(false)
		expect(hit[3] ~= nil).to.equal(false)
		for i = 3, 1, -1 do
			hit[i] = true
			n += 1
		end
		expect(n).to.equal(3)
		expect(hit[1] ~= nil).to.equal(true)
		expect(hit[2] ~= nil).to.equal(true)
		expect(hit[3] ~= nil).to.equal(true)
	end)
	it("should support for-of loops over arrays", function()
		local hit = {}
		local array = { "1", "2", "3", "4" }
		local n = 0
		for _, v in array do
			hit[v] = true
			n += 1
		end
		expect(n).to.equal(4)
		expect(next(difference(hit, {
			["1"] = true,
			["2"] = true,
			["3"] = true,
			["4"] = true,
		})) == nil).to.equal(true)
	end)
	it("should support for-of loops over array literals", function()
		local hit = {}
		local n = 0
		for _, v in { "1", "2", "3", "4" } do
			hit[v] = true
			n += 1
		end
		expect(n).to.equal(4)
		expect(next(difference(hit, {
			["1"] = true,
			["2"] = true,
			["3"] = true,
			["4"] = true,
		})) == nil).to.equal(true)
	end)
	it("should support for-of loops over string literals", function()
		local hit = {}
		local n = 0
		for v in string.gmatch("1234", utf8.charpattern) do
			hit[v] = true
			n += 1
		end
		expect(n).to.equal(4)
		expect(next(difference(hit, {
			["1"] = true,
			["2"] = true,
			["3"] = true,
			["4"] = true,
		})) == nil).to.equal(true)
	end)
	it("should support for-of loops over strings", function()
		local hit = {}
		local str = "1234"
		local n = 0
		for v in string.gmatch(str, utf8.charpattern) do
			hit[v] = true
			n += 1
		end
		expect(n).to.equal(4)
		expect(next(difference(hit, {
			["1"] = true,
			["2"] = true,
			["3"] = true,
			["4"] = true,
		})) == nil).to.equal(true)
	end)
	it("should support for-of loops over computed strings", function()
		local i = 0
		for letter in string.gmatch(`{i}s`, utf8.charpattern) do
		end
	end)
	it("should support for-of loops over Set literals", function()
		local hit = {}
		local n = 0
		for v in {
			["1"] = true,
			["2"] = true,
			["3"] = true,
			["4"] = true,
		} do
			hit[v] = true
			n += 1
		end
		expect(n).to.equal(4)
		expect(next(difference(hit, {
			["1"] = true,
			["2"] = true,
			["3"] = true,
			["4"] = true,
		})) == nil).to.equal(true)
	end)
	it("should support for-of loops over Sets", function()
		local hit = {}
		local set = {
			["1"] = true,
			["2"] = true,
			["3"] = true,
			["4"] = true,
		}
		local n = 0
		for v in set do
			hit[v] = true
			n += 1
		end
		expect(n).to.equal(4)
		expect(next(difference(hit, {
			["1"] = true,
			["2"] = true,
			["3"] = true,
			["4"] = true,
		})) == nil).to.equal(true)
	end)
	it("should support for-of loops over Map literals", function()
		local hit = {}
		local n = 0
		for _k, _v in {
			["1"] = "2",
			["3"] = "4",
		} do
			local v = { _k, _v }
			hit[v[1]] = true
			hit[v[2]] = true
			n += 1
		end
		expect(n).to.equal(2)
		expect(next(difference(hit, {
			["1"] = true,
			["2"] = true,
			["3"] = true,
			["4"] = true,
		})) == nil).to.equal(true)
	end)
	it("should support for-of loops over Maps", function()
		local hit = {}
		local map = {
			["1"] = "2",
			["3"] = "4",
		}
		local n = 0
		for _k, _v in map do
			local v = { _k, _v }
			hit[v[1]] = true
			hit[v[2]] = true
			n += 1
		end
		expect(n).to.equal(2)
		expect(next(difference(hit, {
			["1"] = true,
			["2"] = true,
			["3"] = true,
			["4"] = true,
		})) == nil).to.equal(true)
	end)
	it("should support optimized destructuring in for-of loops over Maps", function()
		local hit = {}
		local map = {
			["1"] = "2",
			["3"] = "4",
		}
		local n = 0
		for k, v in map do
			hit[k] = true
			hit[v] = true
			n += 1
		end
		expect(n).to.equal(2)
		expect(next(difference(hit, {
			["1"] = true,
			["2"] = true,
			["3"] = true,
			["4"] = true,
		})) == nil).to.equal(true)
	end)
	it("should support destructuring optimized destructuring in for-of loops over Maps", function()
		local n = 0
		for _binding, _binding_1 in {
			[{ "1", "2" }] = { "3", "4" },
		} do
			local i = _binding[1]
			local j = _binding[2]
			local k = _binding_1[1]
			local v = _binding_1[2]
			expect(i).to.equal("1")
			expect(j).to.equal("2")
			expect(k).to.equal("3")
			expect(v).to.equal("4")
			n += 1
		end
		expect(n).to.equal(1)
	end)
	it("should support break", function()
		local array = { 1, 2, 3, 4, 5 }
		local sum = 0
		for _, value in array do
			sum += value
			if value == 3 then
				break
			end
		end
		expect(sum).to.equal(6)
	end)
	it("should support continue", function()
		local array = { 1, 2, 3, 4 }
		local sum = 0
		for _, value in array do
			if value == 3 then
				continue
			end
			sum += value
		end
		expect(sum).to.equal(7)
	end)
	it("should support continue with numeric loops", function()
		local x = 0
		for i = 0, 9 do
			if i % 2 == 0 then
				continue
			end
			x += 1
		end
		expect(x).to.equal(5)
	end)
	it("should support do-while loops", function()
		local x = 0
		repeat
			do
				x += 5
			end
		until not (x < 25)
		expect(x).to.equal(25)
		local function expect0(y)
			expect(y).to.equal(0)
			return false
		end
		local y = 0
		repeat
			do
				local y = 1
				expect(y).to.equal(1)
			end
		until not expect0(y)
	end)
	it("should support while loops", function()
		local x = 0
		while x < 10 do
			x += 1
		end
		expect(x).to.equal(10)
	end)
	it("should support for-of destructuring", function()
		local arr = { {
			a = 1,
			b = 2,
			c = 3,
		} }
		for _, _binding in arr do
			local a = _binding.a
			local b = _binding.b
			local c = _binding.c
			expect(a).to.equal(1)
			expect(b).to.equal(2)
			expect(c).to.equal(3)
		end
	end)
	it("should work with gmatch", function()
		for a in string.gmatch("H", ".") do
			expect(a).to.equal("H")
		end
		for a, b, c, d in string.gmatch("Hello!", "(%l)(%l)(%l)(%l)") do
			expect(a).to.equal("e")
			expect(b).to.equal("l")
			expect(c).to.equal("l")
			expect(d).to.equal("o")
		end
	end)
	it("should support indexing tuple as array", function()
		local obj = {
			a = 1,
			b = 2,
			c = 3,
		}
		for _element, _element_1 in pairs(obj) do
			local tuple = { _element, _element_1 }
			expect(tuple[2]).to.equal(obj[tuple[1]])
		end
	end)
	it("should support iterator function with single return when indexing tuple as array", function()
		local shortIterator = (function()
			return true
		end)
		for _value in shortIterator do
			local tuple = { _value }
			expect(#tuple).to.equal(1)
			break
		end
	end)
	it("should support iterator function with multiple returns when indexing tuple as array", function()
		local longIterator = (function()
			return true, true, true, true, true, true
		end)
		for _element, _element_1, _element_2, _element_3, _element_4, _element_5 in longIterator do
			local tuple = { _element, _element_1, _element_2, _element_3, _element_4, _element_5 }
			expect(#tuple).to.equal(6)
			break
		end
	end)
	it("should support iterator function with rest tuple elements when indexing as array", function()
		local callCount = 0
		local restIterator = (function()
			callCount += 1
			if callCount == 1 then
				return 10, 20, 30
			end
			return unpack(nil)
		end)
		local _restIterator = restIterator
		while true do
			local tuple = { _restIterator() }
			if tuple[1] == nil then
				break
			end
			expect(#tuple).to.equal(3)
			expect(tuple[1]).to.equal(10)
			expect(tuple[2]).to.equal(20)
			expect(tuple[3]).to.equal(30)
			break
		end
	end)
	it("should support iterator function with variadic tuple elements when indexing as array", function()
		local function collectFirst(iter)
			local _iter = iter
			while true do
				local entry = { _iter() }
				if entry[1] == nil then
					break
				end
				return entry
			end
			return nil
		end
		local callCount = 0
		local iter = (function()
			callCount += 1
			if callCount == 1 then
				return 10, 20, 30
			end
			return unpack(nil)
		end)
		local result = collectFirst(iter)
		expect(result).to.be.ok()
		expect(#result).to.equal(3)
		expect(result[1]).to.equal(10)
		expect(result[2]).to.equal(20)
		expect(result[3]).to.equal(30)
	end)
	it("should support the $range macro without step", function()
		local hit = {}
		local sum = 10
		for i = 0, 9 do
			hit[i] = true
			sum -= 1
		end
		expect(sum).to.equal(0)
		expect(hit[0] ~= nil).to.equal(true)
		expect(hit[1] ~= nil).to.equal(true)
		expect(hit[2] ~= nil).to.equal(true)
		expect(hit[3] ~= nil).to.equal(true)
		expect(hit[4] ~= nil).to.equal(true)
		expect(hit[5] ~= nil).to.equal(true)
		expect(hit[6] ~= nil).to.equal(true)
		expect(hit[7] ~= nil).to.equal(true)
		expect(hit[8] ~= nil).to.equal(true)
		expect(hit[9] ~= nil).to.equal(true)
	end)
	it("should support the $range macro with a negative step", function()
		local hit = {}
		local sum = 10
		for i = 9, 0, -1 do
			hit[i] = true
			sum -= 1
		end
		expect(sum).to.equal(0)
		expect(hit[0] ~= nil).to.equal(true)
		expect(hit[1] ~= nil).to.equal(true)
		expect(hit[2] ~= nil).to.equal(true)
		expect(hit[3] ~= nil).to.equal(true)
		expect(hit[4] ~= nil).to.equal(true)
		expect(hit[5] ~= nil).to.equal(true)
		expect(hit[6] ~= nil).to.equal(true)
		expect(hit[7] ~= nil).to.equal(true)
		expect(hit[8] ~= nil).to.equal(true)
		expect(hit[9] ~= nil).to.equal(true)
	end)
	it("should support the $range macro with a decimal step", function()
		local hit = {}
		local sum = 19
		for i = 0, 9, 0.5 do
			hit[i] = true
			sum -= 1
		end
		expect(sum).to.equal(0)
		expect(hit[0] ~= nil).to.equal(true)
		expect(hit[0.5] ~= nil).to.equal(true)
		expect(hit[1] ~= nil).to.equal(true)
		expect(hit[1.5] ~= nil).to.equal(true)
		expect(hit[2] ~= nil).to.equal(true)
		expect(hit[2.5] ~= nil).to.equal(true)
		expect(hit[3] ~= nil).to.equal(true)
		expect(hit[3.5] ~= nil).to.equal(true)
		expect(hit[4] ~= nil).to.equal(true)
		expect(hit[4.5] ~= nil).to.equal(true)
		expect(hit[5] ~= nil).to.equal(true)
		expect(hit[5.5] ~= nil).to.equal(true)
		expect(hit[6] ~= nil).to.equal(true)
		expect(hit[6.5] ~= nil).to.equal(true)
		expect(hit[7] ~= nil).to.equal(true)
		expect(hit[7.5] ~= nil).to.equal(true)
		expect(hit[8] ~= nil).to.equal(true)
		expect(hit[8.5] ~= nil).to.equal(true)
		expect(hit[9] ~= nil).to.equal(true)
	end)
end
