# opossum

opossum is a Roblox build tool written in Rust. It compiles TypeScript and Luau into a place, syncs it live to Studio,
uploads assets, deploys through Open Cloud and runs tests, all from one binary.

Current release: **0.1.0**

```ts
export class Inventory {
	private items = new Map<string, number>();

	add(name: string, count = 1) {
		this.items.set(name, (this.items.get(name) ?? 0) + count);
	}

	describe() {
		const parts = new Array<string>();
		for (const [name, count] of this.items) parts.push(`${count}x ${name}`);
		return parts.join(", ");
	}
}
```

```luau
--!strict
--!optimize 2
export type Inventory = { items: { [string]: number }, add: (self: Inventory, name: string, count: number?) -> (), describe: (self: Inventory) -> string }
local Inventory
do
	Inventory = setmetatable({}, {
		__tostring = function()
			return "Inventory"
		end,
	})
	Inventory.__index = Inventory
	function Inventory.new(...): Inventory
		const self: any = setmetatable({}, Inventory)
		return self:constructor(...) or self
	end
	function Inventory:constructor()
		self.items = {}
	end
	function Inventory:add(name: string, count: number?)
		if count == nil then
			count = 1
		end
		const _exp = self.items
		local _condition = self.items[name]
		if _condition == nil then
			_condition = 0
		end
		_exp[name] = _condition + count
	end
	function Inventory:describe(): string
		const parts = {}
		for name, count in self.items do
			table.insert(parts, `{count}x {name}`)
		end
		return table.concat(parts, ", ")
	end
end
return {
	Inventory = Inventory,
}
```

The compiler accepts roblox-ts projects and `@rbxts/*` packages, read straight from `node_modules/` with no Node at
build time. The semantics are TypeScript's; the output is not a copy of roblox-ts's.

## Highlights

- Type checking through tsrs, a Rust port of the TypeScript checker.
- Typed Luau output: TypeScript types become Luau types, `const` locals, `@native` per function.
- Every module passes Luau's own compiler at `-O2` before it ships.
- jecs queries lowered to archetype iteration; Flamework's metadata macros run at compile time.
- Live sync to Studio through a plugin, with no Rojo server. A one-file save reaches Studio in well under a second.
- Place files: services, models (`.rbxm`, `.model.json`) and properties as TOML.
- Assets: images, sounds, meshes and textured `.glb` models uploaded and tracked in `opossum.lock`.
- Passes, products and badges provisioned, places published, all through Open Cloud.
- Specs and benchmarks run locally in fresh Luau VMs, in Studio, or on Open Cloud.

## Build from source

Rust is pinned in `rust-toolchain.toml`. Luau is compiled from source, so a C++17 compiler is needed too. macOS and
Linux.

```sh
cargo install --locked --path crates/opossum
opossum plugin install
```

## Compile a project

```toml
# opossum.toml
name = "my-game"

[routes]
server = "ServerScriptService/server"
client = "ReplicatedStorage/client"
shared = "ReplicatedStorage/shared"
```

Each route maps a folder of `src/` into the place. `.server.ts` and `.client.ts` files become scripts with that
RunContext.

```sh
opossum check
opossum build
opossum dev
```

`build` writes `.opossum/place.rbxl`; `--emit <dir>` also writes the Luau. `dev` watches, rebuilds and syncs to the
place open in Studio.

## Other commands

```text
test        run specs locally, in Studio or on Open Cloud
exec        run Luau in Studio or on Open Cloud
plan        show what upload and provision would change, offline
upload      upload new and changed assets
provision   create and update passes, products and badges
deploy      upload, provision, build and publish
play        start a Studio playtest
pull        write an instance from Studio into the project
where       map a file to its instance, or back
sourcemap   write a sourcemap for luau-lsp
migrate     convert a Rojo, tessera or hexagon project
```

`opossum --help` lists every command and flag. [DESIGN.md](DESIGN.md) covers configuration, routing, the place format,
assets, deploys and tests.

## License

[MIT](LICENSE). Vendored and ported sources retain their upstream licenses and notices, see
[THIRD-PARTY.md](THIRD-PARTY.md).
