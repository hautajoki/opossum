return {
	ambient = {
		entries = function()
			local rows = { { 1, nil }, { 2, { 5 } } }
			local i = 0
			return function()
				i += 1
				local row = rows[i]
				if row then
					return row[1], row[2]
				end
				return nil
			end
		end,
		makeIter = function(count)
			local i = 0
			return function()
				i += 1
				if i <= (count or 0) then
					return i
				end
				return nil
			end
		end,
	},
	calls = {
		{ call = "writableTarget", expect = { "list key0 body key2 body", 10, 20 } },
		{ call = "writableMapTarget", expect = { "map key0 body", "a", 1 } },
		{ call = "nestedDefault", expect = { 1, 7, 2, 5 } },
		{ call = "iterablePrerequisites", expect = { 1, 2 } },
	},
}
