-- Compiled with roblox-ts v3.0.0
-- while and do-while statements
local function whiles(values, text, n, maybe)
	while n > 0 do
		n -= 1
	end
	while true do
		-- ▼ Array.pop ▼
		local _length = #values
		local _result = values[_length]
		values[_length] = nil
		-- ▲ Array.pop ▲
		if not (_result ~= nil) then
			break
		end
		print(#values)
	end
	while text ~= "" and text do
		text = string.sub(text, 2)
	end
	while true do
		local _condition = maybe
		if _condition == nil then
			_condition = n
		end
		if not (_condition ~= 0 and _condition == _condition and _condition) then
			break
		end
		maybe = nil
		n = 0
	end
	while true do
		local _original = n
		n += 1
		if _original > 10 then
			break
		end
		if n % 2 == 0 then
			continue
		end
		print(n)
	end
	while n < 20 do
		n += 1
	end
	while n > 100 do
	end
end
local function doWhiles(values, text, n)
	repeat
		do
			n -= 1
		end
	until not (n > 0)
	repeat
		do
			local x = n
			print(x)
		end
	until (n > 5)
	repeat
		do
			print(#values)
		end
		-- ▼ Array.pop ▼
		local _length = #values
		local _result = values[_length]
		values[_length] = nil
		-- ▲ Array.pop ▲
	until not (_result ~= nil)
	repeat
		do
			text = string.sub(text, 2)
		end
	until not (text ~= "" and text)
	repeat
		do
			if n > 3 then
				continue
			end
			n += 1
		end
	until not (n < 3)
	repeat
		do
			n += 1
		end
	until not (n < 30)
end
return {
	whiles = whiles,
	doWhiles = doWhiles,
}
