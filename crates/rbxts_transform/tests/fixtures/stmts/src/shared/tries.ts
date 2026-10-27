// Try and throw statements: roblox-ts tests' try.spec.ts (without the cases that need loops, arrays or `new`), plus
// hand-written cases.

export = () => {
	it("should preserve a bare return through finally", () => {
		let events = "";
		function run() {
			try {
				events += "try";
				return;
			} finally {
				events += ",finally";
			}
			events += ",after";
		}

		expect(run()).to.equal(undefined);
		expect(events).to.equal("try,finally");
	});

	it("should support try/catch", () => {
		let x: number = 123;
		try {
			x = 456;
		} catch (e) {}
		expect(x).to.equal(456);
	});

	it("should support try/catch with throwing strings", () => {
		function foo(): unknown {
			let exception: unknown;
			try {
				throw "bar";
			} catch (e) {
				exception = e;
			}
			return exception;
		}
		expect(foo()).to.be.a("string");
	});

	it("should support try/catch with return in try block", () => {
		function foo(): unknown {
			try {
				return "foo";
			} catch (e) {}
		}
		expect(foo()).to.equal("foo");
	});

	it("should support try/catch with return in catch block", () => {
		function foo(): unknown {
			try {
				throw undefined;
			} catch {
				return "foo";
			}
		}
		expect(foo()).to.equal("foo");
	});

	it("should support try/catch with return in finally block", () => {
		function foo(): unknown {
			try {
				return 1;
			} finally {
				return 2;
			}
		}
		expect(foo()).to.equal(2);
	});

	it("should support try/catch without a catch variable", () => {
		function foo(): unknown {
			let exception: unknown;
			try {
				throw "bar";
			} catch {
				exception = "abc";
			}
			return exception;
		}
		expect(foo()).to.equal("abc");
	});

	it("should support nested try/catch with return", () => {
		function foo(): unknown {
			try {
				try {
					return "foo";
				} catch (e) {}
			} catch (e) {}
		}
		expect(foo()).to.be.a("string");
	});

	it("should rethrow the error if there's no catch block", () => {
		function foo() {
			try {
				throw "bar";
			} finally {
			}
		}

		expect(() => foo()).to.throw("bar");
	});

	it("should run finally even if catch throws", () => {
		let ranFinally = false;
		try {
			try {
				throw "try error";
			} catch {
				throw "catch error";
			} finally {
				ranFinally = true;
			}
		} catch {}

		expect(ranFinally).to.equal(true);
	});

	it("should throw if finally throws", () => {
		function foo() {
			try {
			} finally {
				throw "bar";
			}
		}

		expect(() => foo()).to.throw("bar");
	});

	it("should discard errors if finally has a control flow statement", () => {
		function tryErrorReturn() {
			try {
				throw "try error";
			} finally {
				return true;
			}
			return false;
		}

		expect(tryErrorReturn()).to.equal(true);

		function catchErrorReturn() {
			try {
				throw "try error";
			} catch {
				throw "catch error";
			} finally {
				return true;
			}
			return false;
		}

		expect(catchErrorReturn()).to.equal(true);
	});

	// hand-written
	it("should return values and tuples through try", () => {
		function pair(): LuaTuple<[number, string]> {
			try {
				return $tuple(1, "one");
			} catch {
				return $tuple(0, "zero");
			}
		}
		function nestedValue(flag: boolean) {
			try {
				try {
					if (flag) {
						return 1;
					}
				} finally {
					print("inner");
				}
				return 2;
			} catch (err) {
				print(err);
				throw err;
			}
		}
		function throwComputed(message: string) {
			throw message + "!";
		}
		function returnsInCallback() {
			try {
				const callback = () => {
					return 3;
				};
				return callback();
			} catch {}
			return 0;
		}

		expect(pair()[0]).to.equal(1);
		expect(nestedValue(true)).to.equal(1);
		expect(() => throwComputed("x")).to.throw("x!");
		expect(returnsInCallback()).to.equal(3);
	});
};
