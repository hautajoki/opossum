-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- Switch statements: roblox-ts tests' switch.spec.ts (without the cases that need arrays, objects, `new` or
-- conditional expressions), plus hand-written cases.
return function()
	it("should support switch statements with fall through", function()
		local function foo(s)
			repeat
				local _fallthrough = false
				if s == "a" then
					_fallthrough = true
				end
				if _fallthrough or s == "b" then
					return 1
				end
				if s == "c" then
					return 2
				end
				if s == "d" then
					if true then
						break
					end
					_fallthrough = true
				end
				if _fallthrough or s == "e" then
					return 4
				end
				return -1
			until true
			return 3
		end
		expect(foo("a")).to.equal(1)
		expect(foo("d")).to.equal(3)
		expect(foo("f")).to.equal(-1)
	end)
	it("should support switch statements without fall through", function()
		local function bar(s)
			repeat
				if s == "a" then
					return 1
				end
				if s == "b" then
					return 2
				end
				if s == "c" then
					return 3
				end
			until true
			return 4
		end
		expect(bar("a")).to.equal(1)
		expect(bar("d")).to.equal(4)
	end)
	it("should evaluate each case expression once", function()
		local evaluations = 0
		local function getCase(value)
			evaluations += 1
			return value
		end
		repeat
			local _fallthrough = false
			local _caseValue = getCase(1)
			if 3 == _caseValue then
				_fallthrough = true
			end
			if not _fallthrough then
				local _caseValue_1 = getCase(2)
				_fallthrough = 3 == _caseValue_1
			end
			if _fallthrough then
				_fallthrough = true
			end
			if not _fallthrough then
				local _caseValue_1 = getCase(3)
				_fallthrough = 3 == _caseValue_1
			end
			if _fallthrough then
				break
			end
			local _caseValue_1 = getCase(4)
			if 3 == _caseValue_1 then
				break
			end
		until true
		expect(evaluations).to.equal(3)
	end)
	it("should evaluate the switch expression only once", function()
		local a = 1
		local matchedAfterMutation = false
		local function increaseA()
			a += 1
			return 0
		end
		local _exp = a
		repeat
			local _caseValue = increaseA()
			if _exp == _caseValue then
				break
			end
			if _exp == 2 then
				matchedAfterMutation = true
				break
			end
		until true
		expect(matchedAfterMutation).to.equal(false)
	end)
	it("should retain the original parameter when case expressions assign to it", function()
		local function assign(value)
			local _exp = value
			repeat
				value = 2
				local _caseValue = value
				if _exp == _caseValue then
					return "two"
				end
				return "original"
			until true
		end
		local function call(value)
			local function change()
				value = 2
				return 0
			end
			local _exp = value
			repeat
				local _caseValue = change()
				if _exp == _caseValue then
					return "zero"
				end
				if _exp == 2 then
					return "two"
				end
				return "original"
			until true
		end
		expect(assign(1)).to.equal("original")
		expect(call(2)).to.equal("two")
	end)
	it("should preserve the operand when a shared case declaration shadows it", function()
		local function classify(value)
			local _exp = value
			repeat
				local _fallthrough = false
				local value
				if _exp == "a" then
					value = "inner"
					_fallthrough = true
				end
				if _fallthrough or _exp == "b" then
					return value
				end
				return "missing"
			until true
		end
		expect(classify("a")).to.equal("inner")
		expect(classify("missing")).to.equal("missing")
	end)
	it("should support switch statements with remaining empty conditions", function()
		local function bar(s)
			repeat
				local _fallthrough = false
				if s == "a" then
					return 1
				end
				if s == "b" then
					_fallthrough = true
				end
				if _fallthrough or s == "c" then
				end
			until true
			return 2
		end
		expect(bar("a")).to.equal(1)
		expect(bar("b")).to.equal(2)
	end)
	it("should support switch statements with context", function()
		local function bar(n)
			local x = 1
			local _exp = n
			repeat
				local _original = x
				x += 1
				if _exp == _original then
					return -1
				end
				local _original_1 = x
				x += 1
				if _exp == _original_1 then
					return -2
				end
			until true
			return 0
		end
		expect(bar(1)).to.equal(-1)
		expect(bar(3)).to.equal(0)
	end)
	it("should support switch statements with fallthrough and context", function()
		local function bar(n)
			local x = 1
			local _exp = n
			repeat
				local _fallthrough = false
				local _original = x
				x += 1
				if _exp == _original then
					_fallthrough = true
				end
				if not _fallthrough then
					local _original_1 = x
					x += 1
					_fallthrough = _exp == _original_1
				end
				if _fallthrough then
					return -2
				end
			until true
			return 0
		end
		expect(bar(1)).to.equal(-2)
	end)
	it("should support switch statements with preceding statements", function()
		local function bar(n)
			local x = 1
			n += 1
			local _exp = n
			repeat
				local _fallthrough = false
				local _original = x
				x += 1
				if _exp == _original then
					_fallthrough = true
				end
				if not _fallthrough then
					local _original_1 = x
					x += 1
					_fallthrough = _exp == _original_1
				end
				if _fallthrough then
					_fallthrough = true
				end
				if not _fallthrough then
					local _original_1 = x
					x += 1
					_fallthrough = _exp == _original_1
				end
				if _fallthrough then
					_fallthrough = true
				end
				if not _fallthrough then
					local _original_1 = x
					x += 1
					_fallthrough = _exp == _original_1
				end
				if _fallthrough then
					return -2
				end
			until true
			return 0
		end
		expect(bar(0)).to.equal(-2)
		expect(bar(4)).to.equal(0)
	end)
	it("should support switch statements with a boolean value", function()
		local function foo(n)
			repeat
				local _caseValue = n >= 10
				if true == _caseValue then
					return 10
				end
				local _caseValue_1 = n >= 5
				if true == _caseValue_1 then
					return 5
				end
				local _caseValue_2 = n >= 0
				if true == _caseValue_2 then
					return 0
				end
			until true
		end
		expect(foo(10)).to.equal(10)
		expect(foo(0)).to.equal(0)
	end)
	-- hand-written
	it("should support default clauses before cases, enum labels and hoisted declarations", function()
		local Shade = {
			Light = "light",
			Dark = "dark",
		}
		local getShade
		local function pick(color, shade, flag)
			local result = 0
			local _exp = color
			repeat
				local _fallthrough = false
				if _exp == 0 then
					result = 1
					break
				end
				local _case
				if _exp == 1 then
					_case = 1
				end
				if _case == nil then
					result = -1
					_fallthrough = true
				end
				if _fallthrough or _case == 1 then
					-- fallthrough into here
					result += 2
				end
			until true
			local _exp_1 = shade
			repeat
				local inner
				if _exp_1 == Shade.Light then
					inner = 5
					result += inner
					break
				end
				if _exp_1 == Shade.Dark then
					inner = 3
					result += inner
					break
				end
			until true
			repeat
				local _fallthrough = false
				if flag == true then
					_fallthrough = true
				end
				if _fallthrough or flag == false then
					result *= 2
				end
			until true
			local _exp_2 = getShade()
			repeat
			until true
			repeat
				result += 1
			until true
			return result
		end
		function getShade()
			return Shade.Dark
		end
		expect(pick(0, Shade.Light, true)).to.equal(13)
	end)
	it("should support breaks and returns through try statements inside switches", function()
		local function run(n)
			local out = 0
			repeat
				local _fallthrough = false
				if n == 1 then
					local _exitType, _returns = TS.try(function()
						out = 1
						return TS.TRY_BREAK
					end, function()
						out = 2
					end)
					if _exitType then
						break
					end
					out = 3
					_fallthrough = true
				end
				if _fallthrough or n == 2 then
					local _exitType, _returns = TS.try(function()
						return TS.TRY_RETURN, { out + 10 }
					end, nil, function()
						out = 20
					end)
					if _exitType then
						return unpack(_returns)
					end
				end
				error("unknown " .. tostring(n))
			until true
			return out
		end
		expect(run(1)).to.equal(1)
		expect(run(2)).to.equal(10)
	end)
end
