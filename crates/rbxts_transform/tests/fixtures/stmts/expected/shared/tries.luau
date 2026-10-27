-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- Try and throw statements: roblox-ts tests' try.spec.ts (without the cases that need loops, arrays or `new`), plus
-- hand-written cases.
return function()
	it("should preserve a bare return through finally", function()
		local events = ""
		local function run()
			local _exitType, _returns = TS.try(function()
				events ..= "try"
				return TS.TRY_RETURN, {}
			end, nil, function()
				events ..= ",finally"
			end)
			if _exitType then
				return unpack(_returns)
			end
			events ..= ",after"
		end
		expect(run()).to.equal(nil)
		expect(events).to.equal("try,finally")
	end)
	it("should support try/catch", function()
		local x = 123
		TS.try(function()
			x = 456
		end, function(e) end)
		expect(x).to.equal(456)
	end)
	it("should support try/catch with throwing strings", function()
		local function foo()
			local exception
			TS.try(function()
				error("bar")
			end, function(e)
				exception = e
			end)
			return exception
		end
		expect(foo()).to.be.a("string")
	end)
	it("should support try/catch with return in try block", function()
		local function foo()
			local _exitType, _returns = TS.try(function()
				return TS.TRY_RETURN, { "foo" }
			end, function(e) end)
			if _exitType then
				return unpack(_returns)
			end
		end
		expect(foo()).to.equal("foo")
	end)
	it("should support try/catch with return in catch block", function()
		local function foo()
			local _exitType, _returns = TS.try(function()
				error(nil)
			end, function()
				return TS.TRY_RETURN, { "foo" }
			end)
			if _exitType then
				return unpack(_returns)
			end
		end
		expect(foo()).to.equal("foo")
	end)
	it("should support try/catch with return in finally block", function()
		local function foo()
			local _exitType, _returns = TS.try(function()
				return TS.TRY_RETURN, { 1 }
			end, nil, function()
				return TS.TRY_RETURN, { 2 }
			end)
			if _exitType then
				return unpack(_returns)
			end
		end
		expect(foo()).to.equal(2)
	end)
	it("should support try/catch without a catch variable", function()
		local function foo()
			local exception
			TS.try(function()
				error("bar")
			end, function()
				exception = "abc"
			end)
			return exception
		end
		expect(foo()).to.equal("abc")
	end)
	it("should support nested try/catch with return", function()
		local function foo()
			local _exitType, _returns = TS.try(function()
				local _exitType_1, _returns_1 = TS.try(function()
					return TS.TRY_RETURN, { "foo" }
				end, function(e) end)
				if _exitType_1 then
					return _exitType_1, _returns_1
				end
			end, function(e) end)
			if _exitType then
				return unpack(_returns)
			end
		end
		expect(foo()).to.be.a("string")
	end)
	it("should rethrow the error if there's no catch block", function()
		local function foo()
			TS.try(function()
				error("bar")
			end, nil, function() end)
		end
		expect(function()
			return foo()
		end).to:throw("bar")
	end)
	it("should run finally even if catch throws", function()
		local ranFinally = false
		TS.try(function()
			TS.try(function()
				error("try error")
			end, function()
				error("catch error")
			end, function()
				ranFinally = true
			end)
		end, function() end)
		expect(ranFinally).to.equal(true)
	end)
	it("should throw if finally throws", function()
		local function foo()
			TS.try(function() end, nil, function()
				error("bar")
			end)
		end
		expect(function()
			return foo()
		end).to:throw("bar")
	end)
	it("should discard errors if finally has a control flow statement", function()
		local function tryErrorReturn()
			local _exitType, _returns = TS.try(function()
				error("try error")
			end, nil, function()
				return TS.TRY_RETURN, { true }
			end)
			if _exitType then
				return unpack(_returns)
			end
			return false
		end
		expect(tryErrorReturn()).to.equal(true)
		local function catchErrorReturn()
			local _exitType, _returns = TS.try(function()
				error("try error")
			end, function()
				error("catch error")
			end, function()
				return TS.TRY_RETURN, { true }
			end)
			if _exitType then
				return unpack(_returns)
			end
			return false
		end
		expect(catchErrorReturn()).to.equal(true)
	end)
	-- hand-written
	it("should return values and tuples through try", function()
		local function pair()
			local _exitType, _returns = TS.try(function()
				return TS.TRY_RETURN, { 1, "one" }
			end, function()
				return TS.TRY_RETURN, { 0, "zero" }
			end)
			if _exitType then
				return unpack(_returns)
			end
		end
		local function nestedValue(flag)
			local _exitType, _returns = TS.try(function()
				local _exitType_1, _returns_1 = TS.try(function()
					if flag then
						return TS.TRY_RETURN, { 1 }
					end
				end, nil, function()
					print("inner")
				end)
				if _exitType_1 then
					return _exitType_1, _returns_1
				end
				return TS.TRY_RETURN, { 2 }
			end, function(err)
				print(err)
				error(err)
			end)
			if _exitType then
				return unpack(_returns)
			end
		end
		local function throwComputed(message)
			error(message .. "!")
		end
		local function returnsInCallback()
			local _exitType, _returns = TS.try(function()
				local callback = function()
					return 3
				end
				return TS.TRY_RETURN, { callback() }
			end, function() end)
			if _exitType then
				return unpack(_returns)
			end
			return 0
		end
		expect((pair())).to.equal(1)
		expect(nestedValue(true)).to.equal(1)
		expect(function()
			return throwComputed("x")
		end).to:throw("x!")
		expect(returnsInCallback()).to.equal(3)
	end)
end
