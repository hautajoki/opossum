-- Compiled with roblox-ts v3.0.0
local TS = require(game:GetService("ReplicatedStorage"):WaitForChild("include"):WaitForChild("RuntimeLib"))
-- Hand-written JSX fixture: JSX in a server script.
local React = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "react")
local Button = TS.import(script, game:GetService("ReplicatedStorage"), "shared", "elements").Button
local tree = React.createElement(Button, {
	Text = "server",
	Order = 1,
})
print(tree)
