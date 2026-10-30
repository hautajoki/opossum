# opossum

opossum is a Roblox build tool written in Rust. It compiles TypeScript and Luau into a place, syncs it live to Studio,
uploads assets, deploys through Open Cloud and runs tests, all from one binary.

Current release: **0.1.0**

```ts
export type Shape =
	| { kind: "circle"; radius: number }
	| { kind: "rect"; width: number; height: number };

export const area = (shape: Shape) =>
	shape.kind === "circle" ? math.pi * shape.radius ** 2 : shape.width * shape.height;

export const largest = <T extends Shape>(shapes: ReadonlyArray<T>) => {
	let best: T | undefined;
	for (const shape of shapes) if (!best || area(shape) > area(best)) best = shape;
	return best;
};
```

```luau
--!strict
--!optimize 2
export type Shape = { kind: "circle", radius: number } | { kind: "rect", width: number, height: number }
const area = function(shape: Shape): number
	return if shape.kind == "circle" then math.pi * shape.radius ^ 2 else shape.width * shape.height
end
const largest = function<T>(shapes: {T})
	local best: T?
	for _, shape in shapes do
		if not best or area(shape) > area(best) then
			best = shape
		end
	end
	return best
end
return {
	area = area,
	largest = largest,
}
```

Unions, literal types, generics and inferred return types carry over into Luau's type system. The compiler accepts
roblox-ts projects and `@rbxts/*` packages, read straight from `node_modules/` with no Node at build time. The
semantics are TypeScript's; the output is not a copy of roblox-ts's.

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
git clone https://github.com/hautajoki/opossum
cd opossum
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

`opossum --help` lists every command and flag.

## License

[MIT](LICENSE). Vendored and ported sources retain their upstream licenses and notices.
