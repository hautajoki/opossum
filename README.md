# opossum

opossum takes a Roblox game written in TypeScript (the roblox-ts dialect) and Luau, compiles it, builds the place and
syncs it live to Studio. The same binary uploads the game's assets, provisions its passes, products and badges, deploys
its places through Open Cloud, and runs its tests locally, in Studio or on Open Cloud.

It stands in for roblox-ts (or typescript-go-roblox), Rojo, rogen, tessera and hexagon. Building needs no Node; npm
packages are only read from `node_modules/`.

## Example

```ts
// src/shared/bag.ts
interface Item {
	name: string;
	weight: number;
}

export class Bag {
	private items = new Array<Item>();

	constructor(readonly capacity: number) {}

	add(item: Item) {
		if (this.weight() + item.weight > this.capacity) return false;
		this.items.push(item);
		return true;
	}

	weight() {
		let total = 0;
		for (const { weight } of this.items) total += weight;
		return total;
	}

	describe() {
		return `${this.items.size()} items, ${this.weight()}/${this.capacity} kg`;
	}
}

// src/server/main.server.ts
import { Players } from "@rbxts/services";
import { Bag } from "../shared/bag";

Players.PlayerAdded.Connect((player) => {
	const bag = new Bag(20);
	bag.add({ name: "lantern", weight: 3 });
	print(`${player.Name}: ${bag.describe()}`);
	player.Character?.PivotTo(new CFrame(0, 10, 0));
});
```

`opossum build --emit out` writes this, unedited:

```luau
-- out/ReplicatedStorage/shared/bag.luau
--!strict
--!optimize 2
type Item = { name: string, weight: number }
export type Bag = { items: {Item}, read capacity: number, add: (self: Bag, item: Item) -> boolean, weight: (self: Bag) -> number, describe: (self: Bag) -> string }
local Bag
do
	Bag = setmetatable({}, {
		__tostring = function()
			return "Bag"
		end,
	})
	Bag.__index = Bag
	function Bag.new(...): Bag
		const self: any = setmetatable({}, Bag)
		return self:constructor(...) or self
	end
	function Bag:constructor(capacity: number)
		self.capacity = capacity
		self.items = {}
	end
	function Bag:add(item: Item): boolean
		if self:weight() + item.weight > self.capacity then
			return false
		end
		table.insert(self.items, item)
		return true
	end
	function Bag:weight(): number
		local total = 0
		for _, _binding in self.items do
			const weight = _binding.weight
			total += weight
		end
		return total
	end
	function Bag:describe(): string
		return `{#self.items} items, {self:weight()}/{self.capacity} kg`
	end
end
return {
	Bag = Bag,
}

-- out/ServerScriptService/server/main.luau
--!strict
--!optimize 2
const Players = game:GetService("Players")
const Bag = require("@game/ReplicatedStorage/shared/bag").Bag
Players.PlayerAdded:Connect(function(player: Player)
	const bag = Bag.new(20)
	bag:add({
		name = "lantern",
		weight = 3,
	})
	print(`{player.Name}: {bag:describe()}`)
	const _result = player.Character
	if _result ~= nil then
		_result:PivotTo(CFrame.new(0, 10, 0))
	end
end)
return nil
```

TypeScript types become Luau types, services become `GetService` locals, and each require is spelled from where both
modules land in the place. Every module goes through Luau's own compiler at `-O2` before it ships. jecs
`for (const ... of world.query(...))` loops lower to archetype iteration, and Flamework's metadata macros are evaluated
at compile time; the rest of Flamework is not supported.

## Install

```bash
cargo install --locked --path crates/opossum
opossum plugin install    # the Studio plugin, ~/Documents/Roblox/Plugins/opossum.rbxm
```

`rust-toolchain.toml` pins Rust 1.99.1, which rustup fetches on the first build. Luau is compiled from source, so a
C++17 compiler is needed too. macOS and Linux; there is no Windows support.

## A project

```
opossum.toml
tsconfig.json         { "extends": "./.opossum/tsconfig.json" }
src/server/           -> ServerScriptService/server
src/client/           -> ReplicatedStorage/client
src/shared/           -> ReplicatedStorage/shared
place/                services, models (.rbxm, .model.json) and properties (place/*.toml)
node_modules/@rbxts/  types and compiler-types, from your package manager
```

```toml
name = "Demo"

[routes]
server = "ServerScriptService/server"
client = "ReplicatedStorage/client"
shared = "ReplicatedStorage/shared"
```

A folder or an `@key` suffix (`save@server.ts`) routes a file. `.server.ts` and `.client.ts` become scripts with that
RunContext. With no `placeId` or `[environments]` in the config, `opossum dev` syncs to any unpublished place open in
Studio.

```bash
opossum dev              # watch, rebuild and sync to Studio
opossum build            # .opossum/place.rbxl; -o for another path, --emit <dir> for the Luau
opossum check            # type errors, routing errors and the O2 gate, no place written
opossum test             # specs in Studio through dev; --local in fresh Luau VMs; --cloud on Open Cloud
opossum exec -e 'print(workspace.Gravity)'    # Luau in Studio, after dev has synced every saved file
opossum plan             # what upload and provision would do, offline
opossum upload           # new and changed assets up, ids into opossum.lock
opossum deploy           # upload, provision, then build and publish every place
opossum migrate          # convert a Rojo, tessera or hexagon project once
```

`opossum --help` lists every command and flag.

## How it differs

The whole project is one in-memory graph, and Studio patches, the `.rbxl`, the luau-lsp sourcemap and `where` are views
of it. On a 1,107-module game, a one-file save is republished in 0.11 to 0.16 ms and reaches Studio in 50 to 250 ms,
almost all of that the macOS file watcher. An `exec` sent right after a save runs the saved code, about 65 ms end to
end. `test --local` runs 1,107 local cases in 5.5 s, where hexagon took 9.9 s. `plan` over 1,223 asset files
takes 0.07 s, and a deploy with nothing to upload 1.2 s.

The output does not try to match roblox-ts. It aims at the same TypeScript semantics in faster, more readable Luau:
`--!strict` with native types, `const` locals, numeric `for` where it is provably safe, `@native` per function instead
of per file.

## Status

opossum is 0.1.0.
The Open Cloud paths (upload, provision, deploy, `--cloud` runs) are tested against a mock of the API that ships with
it (`cargo run -p cloud --example mock`), and a full deploy of 1,300 assets has run against that mock.
Changes are hard cutovers with no compatibility layers, so expect configs and commands to break between versions.

DESIGN.md covers the rest: configuration, routing, the `place/` format, assets and the lock file, deploys, tests, exit
codes and the JSON output. `crates/sync/SYNC.md` is the Studio sync protocol.

## License

MIT OR Apache-2.0, at your option. Vendored and ported code (`vendor/`, the runtime, parts of the lowering and the VM)
keeps its own license; see THIRD-PARTY.md.
